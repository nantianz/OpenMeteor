"""Read-only offline GMDL scene and vectorized BVH ray queries (model Z-up).

Layout follows meteor-asset/src/gmdl.rs; geometry is already in model space.
No original files are modified. Only static opaque, visible scene batches bake.
"""
import re
import struct
from pathlib import Path
import numpy as np

ROOT = Path(__file__).resolve().parents[2]


def read_scene(stage):
    path = ROOT / 'origin_res' / stage / (stage + '.gmb')
    data = path.read_bytes()
    assert data[:10] == b'GMDL V1.00'
    offset = 10

    def take(fmt):
        nonlocal offset
        values = struct.unpack_from('<' + fmt, data, offset)
        offset += struct.calcsize('<' + fmt)
        return values[0] if len(values) == 1 else values

    def string():
        nonlocal offset
        size = take('I')
        value = data[offset:offset + size].decode('gb18030')
        offset += size
        return value

    textures = [string() for _ in range(take('I'))]
    materials = [(take('i'), string(), take('B'), string(), take('f'))
                 for _ in range(take('I'))]
    _, _, total, _ = take('4I')
    vertices = np.dtype([('p', '<f4', 3), ('n', '<f4', 3), ('c', 'u1', 4), ('uv', '<f4', 2)])
    faces = np.dtype([('m', '<i4'), ('i', '<u4', 3), ('n', '<f4', 3)])
    objects = []
    count = 0
    while count < total:
        name = string()
        nv, nf = take('2I')
        v = np.frombuffer(data, vertices, nv, offset)
        offset += nv * 36
        f = np.frombuffer(data, faces, nf, offset)
        offset += nf * 28
        count += nv
        objects.append((name, v, f))
    assert count == total
    des = path.with_suffix('.des').read_text(encoding='gb18030')
    bodies = dict(re.findall(r'Object\s+(\S+)\s*\{(.*?)\n\}', des, re.S))
    return textures, materials, objects, bodies


def static_triangles(scene, skybox=()):
    textures, materials, objects, bodies = scene
    sky_names = {(target['object'].lower(), target['texture'].lower()) for target in skybox}
    positions, uvs, colors, ids, sky = [], [], [], [], []
    for name, v, faces in objects:
        body = bodies.get(name, '')
        if re.search(r'visible\s*=\s*0|billboard\s*=\s*1|animation\s*=|camerabox', body, re.I):
            continue
        for m in np.unique(faces['m']):
            if m < 0 or m >= len(materials):
                continue
            material = materials[m]
            # Do not freeze moving smoke, clouds, waterfalls, alpha masks or water.
            if not material[3].upper().startswith('DISABLE') or material[1] != 'NORMAL':
                continue
            indices = faces['i'][faces['m'] == m]
            positions.extend(v['p'][indices])
            uvs.extend(v['uv'][indices])
            colors.extend(v['c'][indices, :3] / 255.0)
            ids.extend([m] * len(indices))
            texture = material[0]
            sky.extend([texture >= 0 and (name.lower(), textures[texture].lower()) in sky_names] * len(indices))
    return tuple(np.asarray(v) for v in (positions, uvs, colors, ids, sky))


class RayScene:
    def __init__(self, triangles):
        self.triangles = np.asarray(triangles, dtype=np.float64)
        self.lo = self.triangles.min(1)
        self.hi = self.triangles.max(1)
        self.root = self._build(np.arange(len(triangles)))

    def _build(self, ids):
        lo, hi = self.lo[ids].min(0), self.hi[ids].max(0)
        if len(ids) <= 16:
            return lo, hi, ids, None
        centers = (self.lo[ids] + self.hi[ids]) * 0.5
        axis = np.ptp(centers, axis=0).argmax()
        ids = ids[np.argsort(centers[:, axis], kind='stable')]
        middle = len(ids) // 2
        return lo, hi, self._build(ids[:middle]), self._build(ids[middle:])

    def trace(self, origins, directions):
        origins, directions = np.broadcast_arrays(origins, directions)
        origins, directions = origins.reshape(-1, 3), directions.reshape(-1, 3)
        count = len(origins)
        distance = np.full(count, np.inf)
        hit = np.full(count, -1, dtype=int)
        bary = np.zeros((count, 3))
        inverse = 1.0 / np.where(np.abs(directions) < 1e-12, 1e-12, directions)

        def visit(node, rays):
            lo, hi, left, right = node
            a = (lo - origins[rays]) * inverse[rays]
            b = (hi - origins[rays]) * inverse[rays]
            near = np.maximum(np.minimum(a, b).max(1), 0)
            far = np.minimum(np.maximum(a, b).min(1), distance[rays])
            rays = rays[near <= far + 1e-8]
            if not len(rays):
                return
            if right is not None:
                visit(left, rays)
                visit(right, rays)
                return
            o, d = origins[rays], directions[rays]
            for index in left:
                a, b, c = self.triangles[index]
                e1, e2 = b - a, c - a
                p = np.cross(d, e2)
                det = np.einsum('ij,j->i', p, e1)
                inv = np.divide(1.0, det, out=np.zeros_like(det), where=np.abs(det) > 1e-9)
                tvec = o - a
                u = np.einsum('ij,ij->i', tvec, p) * inv
                q = np.cross(tvec, e1)
                v = np.einsum('ij,ij->i', d, q) * inv
                t = np.einsum('ij,j->i', q, e2) * inv
                valid = (np.abs(det) > 1e-9) & (u >= -1e-7) & (v >= -1e-7) & (u + v <= 1.0000001) & (t > 0.001) & (t < distance[rays])
                selected = rays[valid]
                distance[selected], hit[selected] = t[valid], index
                bary[selected] = np.stack((1 - u[valid] - v[valid], u[valid], v[valid]), axis=1)
        visit(self.root, np.arange(count))
        return distance, hit, bary
