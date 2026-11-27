#!/usr/bin/env python3
"""Bake small static water reflection/depth maps; Python + NumPy + Pillow.

Reflection captures only original visible opaque scene triangles with their UVs
and vertex colors. No invented sky on rays that miss geometry (alpha=0).
Depth is the measured distance from each water triangle to opaque geometry below;
missing bed samples are explicitly invalid, never treated as a fictional floor.
"""
import argparse
from collections import Counter
import hashlib
import io
import json
import zipfile
from pathlib import Path
import numpy as np
from PIL import Image, ImageFilter
from water_bake_scene import ROOT, RayScene, read_scene, static_triangles

OUT = ROOT / 'asset-tools/generated/water'
WATER_PACK = ROOT / 'meteor_rust/config/graphics-packs/water'
GRAPHICS = ROOT / 'meteor_rust/config/graphics-enhancement.json'
FACE = 64
TILE = FACE + 2


def png(pixels):

    image = Image.fromarray(np.uint8(np.clip(np.rint(pixels), 0, 255)))
    stream = io.BytesIO()
    image.transpose(Image.Transpose.FLIP_TOP_BOTTOM).save(stream, format='PNG', compress_level=9)
    return stream.getvalue()


def cube_directions(face_size=FACE):
    tile_size = face_size + 2
    q = ((np.arange(tile_size) - 1 + 0.5) / face_size) * 2 - 1
    u, v = np.meshgrid(q, q)
    one = np.ones_like(u)
    return [np.stack(parts, -1).reshape(-1, 3) for parts in
            [(one, -v, -u), (-one, -v, u), (u, one, v),
             (u, -one, -v), (u, -v, one), (-u, -v, -one)]]


def water_sky_visibility(water, scene, sky, lo, hi, cells, sectors):
    """Pack four directional skyline heights per RGBA tile."""
    yy, xx = np.mgrid[0:cells, 0:cells]
    uv = (np.stack((xx, yy), -1).reshape(-1, 2) + .5) / cells
    origins = np.column_stack((lo[0] + uv[:, 0] * (hi[0] - lo[0]),
                               hi[1] - uv[:, 1] * (hi[1] - lo[1]),
                               np.full(len(uv), hi[2] + 10)))
    height, water_hit, _ = water.trace(origins, [0, 0, -1])
    wet = water_hit >= 0
    origins[wet, 2] -= height[wet] - 1.0

    def blocked(origins, direction):
        _, hit, _ = scene.trace(origins, direction)
        return (hit >= 0) & ~sky[np.maximum(hit, 0)]



    opened = np.flatnonzero(wet)
    opened = opened[~blocked(origins[opened], [0, 0, 1])]
    origins = origins[opened]
    atlas = np.full((cells, sectors // 4 * cells, 4), 255.0)
    for index in range(sectors):
        angle = 2 * np.pi * index / sectors
        axis = np.array([np.cos(angle), -np.sin(angle)])

        def ray(elevation):
            return np.column_stack((np.sqrt(1 - elevation**2)[:, None] * axis, elevation))

        lower = np.zeros(len(opened))
        upper = np.ones(len(opened))
        remaining = np.arange(len(opened))
        for elevation in (.94, .78, .57, .38, .22, .10, .03):
            angles = np.full(len(remaining), elevation)
            hit = blocked(origins[remaining], ray(angles))
            lower[remaining[hit]] = elevation
            upper[remaining[~hit]] = elevation
            remaining = remaining[~hit]
            if not len(remaining):
                break
        for _ in range(3):
            midpoint = (lower + upper) * .5
            hit = blocked(origins, ray(midpoint))
            lower = np.where(hit, midpoint, lower)
            upper = np.where(hit, upper, midpoint)
        atlas[opened // cells, opened % cells + (index // 4) * cells, index % 4] = upper * 255
    return png(atlas)


def normals():
    y, x = np.mgrid[0:128, 0:128] * (2 * np.pi / 128)
    dx, dy = np.zeros_like(x), np.zeros_like(y)



    vectors = [(1, 2), (2, -3), (-3, 1), (4, 1), (-1, 4), (3, 3),
               (-4, 2), (2, 5), (-5, -2), (5, -1), (4, -4), (-3, -5)]
    for i, (kx, ky) in enumerate(vectors):
        length = np.hypot(kx, ky)
        amplitude = 0.07 / length ** 1.25
        phase = (i * i * 1.6180339 + i * .37 + .13) * 2 * np.pi
        c = np.cos(kx * x + ky * y + phase)
        dx += kx * amplitude * c
        dy += ky * amplitude * c
    n = np.stack((-dx, -dy, np.ones_like(x)), -1)
    n /= np.linalg.norm(n, axis=-1, keepdims=True)
    return png((n * .5 + .5) * 255)


def shoreline_distance(triangles, points):
    """Distance in original model units to the actual liquid polygon boundary."""
    edge_counts = Counter(
        tuple(sorted((tuple(a), tuple(b))))
        for triangle in triangles
        for a, b in ((triangle[0], triangle[1]), (triangle[1], triangle[2]),
                     (triangle[2], triangle[0]))
    )
    xy = np.asarray(points, dtype=np.float64)[:, :2]
    distance = np.full(len(xy), np.inf)
    for (a, b), count in edge_counts.items():
        if count != 1:
            continue
        a, b = np.asarray(a[:2], dtype=np.float64), np.asarray(b[:2], dtype=np.float64)
        edge = b - a
        length_sq = edge[0] * edge[0] + edge[1] * edge[1]
        if length_sq < 1e-12:
            continue
        projection = (xy[:, 0] - a[0]) * edge[0] + (xy[:, 1] - a[1]) * edge[1]
        along = np.clip(projection / length_sq, 0.0, 1.0)
        distance = np.minimum(distance, np.linalg.norm(xy - (a + along[:, None] * edge), axis=1))
    return distance


class Textures:
    def __init__(self, stage):
        self.entries = {}
        self.sources = {}
        self.cache = {}
        for name in [stage + '.pak', 'TexturePatch.pak', 'ctexture.pak', 'ptexture.pak']:
            path = ROOT / 'origin_res' / name
            if not path.exists():
                continue
            with zipfile.ZipFile(path) as pak:
                for entry in pak.namelist():
                    base = entry.replace('\\', '/').split('/')[-1].lower()
                    if base not in self.entries:
                        self.entries[base] = (path, entry)

    def get(self, name):
        base = name.replace('\\', '/').split('/')[-1].lower()
        if base not in self.cache:
            match = self.entries.get(base)
            if match is None:
                self.cache[base] = np.full((1, 1, 3), 255.0)
                self.sources[base] = {'missing': True}
            else:
                path, entry = match
                with zipfile.ZipFile(path) as pak:
                    raw = pak.read(entry)
                self.cache[base] = np.asarray(Image.open(io.BytesIO(raw)).convert('RGB'), dtype=float)
                self.sources[base] = {'pak': path.name, 'entry': entry, 'sha256': hashlib.sha256(raw).hexdigest()}
        return self.cache[base]

    def sample(self, name, uv):
        pixels = self.get(name)
        h, w = pixels.shape[:2]

        x = (uv[:, 0] % 1) * w - .5
        y = ((1 - uv[:, 1]) % 1) * h - .5
        ix, iy = np.floor(x).astype(int), np.floor(y).astype(int)
        fx, fy = (x - ix)[:, None], (y - iy)[:, None]
        return ((pixels[iy % h, ix % w] * (1 - fx) + pixels[iy % h, (ix + 1) % w] * fx) * (1 - fy)
                + (pixels[(iy + 1) % h, ix % w] * (1 - fx) + pixels[(iy + 1) % h, (ix + 1) % w] * fx) * fy)


def select_water_sky(source, scenes):
    sky, reflection = None, None
    if source['kind'] != 1:
        return None
    for scene in scenes:
        if scene['stage'].lower() not in ('*', source['stage'].lower()):
            continue
        if scene.get('sky') is not None:
            sky = scene['sky']
        for entry in scene.get('water_sky_reflections', []):
            if all(entry['surface'][key].lower() == source[key].lower() for key in ('object', 'texture')):
                reflection = entry
    if reflection and reflection['enabled'] and sky and sky.get('enabled', False) and sky.get('replace_skybox', False):
        cells, sectors = (reflection[key] for key in ('visibility_resolution', 'visibility_directions'))
        if not (16 <= cells <= 96 and 4 <= sectors <= 32 and sectors % 4 == 0):
            raise ValueError(f'Invalid skyline grid: {cells} × {sectors}')
        return (cells, sectors, sky['skybox'])
    return None


def bake(source, skyline):
    stage, name = source['stage'], source['object']
    scene = read_scene(stage)
    textures, materials, objects, bodies = scene
    _, vertices, faces = next(obj for obj in objects if obj[0] == name)
    selected = [i for i, m in enumerate(materials) if i in faces['m'] and m[0] >= 0 and textures[m[0]].lower() == source['texture'].lower()]
    blend_mode = source.get('blend_mode', 'src-alpha')
    assert len(selected) == 1 and materials[selected[0]][3].lower() == {
        'src-alpha': 'enable sa 1-sa',
        'src-color-one': 'enable s 1',
    }[blend_mode]
    triangles = vertices['p'][faces['i'][np.isin(faces['m'], selected)]].astype(float)
    assert len(triangles) and 'TextureAnimation: 1' in bodies[name]
    if source.get('kind', 1) == 3 or blend_mode == 'src-color-one':


        lo, hi = triangles.reshape(-1, 3).min(0), triangles.reshape(-1, 3).max(0)
        evidence = {**source, 'bounds_model_world_axes': [lo[0]*.12, -hi[1]*.12, (hi[0]-lo[0])*.12, (hi[1]-lo[1])*.12],
                    'vertex_alpha': float(vertices['c'][faces['i'][np.isin(faces['m'], selected)],3].max()/255),
                    'water_triangles': len(triangles), 'bed_coverage': None,
                    'inputs': {suffix: hashlib.sha256((ROOT/'origin_res'/stage/(stage+suffix)).read_bytes()).hexdigest() for suffix in ['.gmb','.des']}}
        return None, None, None, evidence
    geometry, uvs, colors, ids, sky = static_triangles(scene, skyline[2] if skyline else ())
    tracer = RayScene(geometry)
    tex = Textures(stage)

    areas = np.linalg.norm(np.cross(triangles[:, 1] - triangles[:, 0], triangles[:, 2] - triangles[:, 0]), axis=1)
    candidates = triangles.mean(1) + [0, 0, 2]
    overhead, _, _ = tracer.trace(candidates, np.array([0, 0, 1]))

    clearance = np.minimum(overhead, 10000)
    open_water = clearance >= min(100, clearance.max() * .8)
    probe = candidates[np.where(open_water, areas, -1).argmax()]
    face_size = source.get('reflection_face', FACE)
    tile_size = face_size + 2
    atlas = np.zeros((tile_size * 2, tile_size * 3, 4))
    for face, world in enumerate(cube_directions(face_size)):
        model = world[:, [0, 2, 1]] * [1, -1, 1]
        _, hit, weights = tracer.trace(probe, model)
        pixels = np.zeros((len(hit), 4))
        for material in np.unique(ids[hit[hit >= 0]]):
            valid = (hit >= 0) & (ids[np.maximum(hit, 0)] == material)
            tri = hit[valid]
            uv = np.sum(uvs[tri] * weights[valid, :, None], axis=1)
            color = np.sum(colors[tri] * weights[valid, :, None], axis=1)
            texture = materials[material][0]
            pixels[valid, :3] = color * (tex.sample(textures[texture], uv) if texture >= 0 else 255)
            pixels[valid, 3] = 255
        tile = np.asarray(Image.fromarray(np.uint8(np.clip(pixels.reshape(tile_size, tile_size, 4), 0, 255))).filter(ImageFilter.GaussianBlur(.65)))
        atlas[(face // 3) * tile_size:(face // 3 + 1) * tile_size, (face % 3) * tile_size:(face % 3 + 1) * tile_size] = tile

    lo, hi = triangles.reshape(-1, 3).min(0), triangles.reshape(-1, 3).max(0)
    bounds = [lo[0] * .12, -hi[1] * .12, (hi[0] - lo[0]) * .12, (hi[1] - lo[1]) * .12]
    yy, xx = np.mgrid[0:128, 0:128]
    origins = np.stack((lo[0] + (xx + .5) / 128 * (hi[0] - lo[0]),
                        hi[1] - (yy + .5) / 128 * (hi[1] - lo[1]), np.full_like(xx, hi[2] + 10)), -1).reshape(-1, 3)
    water = RayScene(triangles)
    water_distance, water_hit, _ = water.trace(origins, np.array([0, 0, -1]))
    valid_water = water_hit >= 0
    origins[valid_water, 2] -= water_distance[valid_water] + .01
    depth, bed, _ = tracer.trace(origins, np.array([0, 0, -1]))
    valid = valid_water & (bed >= 0)
    pixels = np.zeros((len(depth), 4))
    pixels[:, 0] = np.where(valid, np.minimum(depth * .12 / source['max_depth'], 1), 0) * 255
    pixels[:, 1] = valid * 255
    pixels[:, 2] = valid_water * 255



    shore = shoreline_distance(triangles, origins)
    pixels[:, 3] = np.where(valid_water, np.minimum(shore * .12 / 12.0, 1.0), 0) * 255
    evidence = {**source, 'probe_model': probe.tolist(), 'bounds_model_world_axes': bounds,
                'static_triangles': len(geometry), 'water_triangles': len(triangles),
                'vertex_alpha': float(vertices['c'][faces['i'][np.isin(faces['m'], selected)], 3].max() / 255.0),
                'bed_coverage': float(valid.sum() / max(1, valid_water.sum())),
                'textures': tex.sources,
                'inputs': {suffix: hashlib.sha256((ROOT / 'origin_res' / stage / (stage + suffix)).read_bytes()).hexdigest() for suffix in ['.gmb', '.des']}}
    print(f'{stage}/{name}: {len(geometry)} static triangles; bed coverage {evidence["bed_coverage"]:.1%}', flush=True)
    visibility = water_sky_visibility(water, tracer, sky, lo, hi, *skyline[:2]) if skyline else None
    return png(atlas), png(pixels.reshape(128, 128, 4)), visibility, evidence


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--check', action='store_true')
    parser.add_argument('--graphics-config', type=Path, default=GRAPHICS)
    args = parser.parse_args()
    index = json.loads(args.graphics_config.read_text(encoding='utf-8'))
    scenes = [scene for pack in index['packs']
              for scene in json.loads((args.graphics_config.parent / pack).read_text(encoding='utf-8')).get('scenes', [])]
    expected = {'ripples.png': normals()}
    manifest = {'version': 1, 'algorithm': 'static-opaque-bvh-variable-cube-depth128-shore12-v3', 'surfaces': []}
    catalog = []
    for entry in json.loads((WATER_PACK / 'water-enhancement.json').read_text(encoding='utf-8'))['surfaces']:
        if not entry.get('enabled', True):
            continue
        source = {key: value for key, value in entry.items() if key != 'enabled'}
        if source.get('reflection_face') == 64:
            source.pop('reflection_face')
        if source.get('blend_mode') == 'src-alpha':
            source.pop('blend_mode')
        skyline = select_water_sky(source, scenes)
        reflection, depth, visibility, evidence = bake(source, skyline)
        prefix = source['stage'] + '-' + source['object']
        if reflection is not None:
            expected[prefix + '-reflection.png'], expected[prefix + '-depth.png'] = reflection, depth
        if visibility is not None:
            expected[prefix + '-sky-visibility.png'] = visibility
        manifest['surfaces'].append(evidence)
        entry = {key: evidence[key] for key in (
            'stage', 'object', 'texture', 'kind', 'max_depth', 'vertex_alpha')}
        entry['additive'] = source.get('blend_mode') == 'src-color-one'
        entry['bounds'] = evidence['bounds_model_world_axes']
        entry['reflection_face'] = source.get('reflection_face', FACE)
        if reflection is not None:
            entry['reflection'] = f'baked/{prefix}-reflection.png'
            entry['depth'] = f'baked/{prefix}-depth.png'
        if visibility is not None:
            cells, sectors, skybox = skyline
            entry['sky_visibility'] = {
                'file': f'baked/{prefix}-sky-visibility.png',
                'layout': [cells, sectors],
                'skybox': list(skybox),
            }
        catalog.append(entry)
    expected['catalog.json'] = (json.dumps(catalog, indent=2) + '\n').encode()
    manifest['outputs'] = {name: hashlib.sha256(data).hexdigest() for name, data in expected.items()}
    outputs = {WATER_PACK / 'baked' / name: data for name, data in expected.items()}
    outputs[OUT / 'manifest.json'] = (json.dumps(manifest, indent=2) + '\n').encode()
    for target, data in outputs.items():
        if args.check:
            if target != OUT / 'manifest.json' or target.exists():
                assert target.read_bytes() == data, f'Stale water bake: {target}'
        else:
            target.parent.mkdir(parents=True, exist_ok=True)
            target.write_bytes(data)
    print(('Verified' if args.check else 'Generated') + f' {len(expected)} files, {sum(len(d) for n,d in expected.items() if n.endswith(".png"))} PNG bytes')


if __name__ == '__main__':
    main()
