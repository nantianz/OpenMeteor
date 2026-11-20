#!/usr/bin/env python3
"""Offline regression: actual bed distances, misses, cube orientation and normals."""
import unittest
import numpy as np
from PIL import Image
import io
from water_bake_scene import ROOT, RayScene, read_scene, static_triangles
from generate_water import cube_directions, normals, shoreline_distance, FACE, TILE


class WaterBakeTests(unittest.TestCase):
    def test_depth_hit_miss_and_nearest_surface(self):
        bed = np.array([[[-2,-2,-3],[2,-2,-3],[0,2,-3]],
                        [[-2,-2,-5],[2,-2,-5],[0,2,-5]]], dtype=float)
        distance, hit, bary = RayScene(bed).trace(np.array([[0,0,1],[4,4,1]]), np.array([0,0,-1]))
        self.assertAlmostEqual(distance[0], 4)
        self.assertEqual(hit.tolist(), [0,-1])
        self.assertTrue(np.isinf(distance[1]))
        np.testing.assert_allclose(bary[0].sum(), 1)
        np.testing.assert_allclose((bed[0] * bary[0,:,None]).sum(0), [0,0,-3])

    def test_cube_axes_and_padded_edges(self):
        axes = np.array([[1,0,0],[-1,0,0],[0,1,0],[0,-1,0],[0,0,1],[0,0,-1]])
        for i, directions in enumerate(cube_directions()):
            q = directions.reshape(TILE,TILE,3)
            self.assertEqual(q.shape, (66,66,3))
            np.testing.assert_allclose(q[32:34,32:34].mean((0,1)), axes[i], atol=1e-12)
            self.assertGreater(np.abs(q).max(), 1)  # filtering gutter traces adjacent directions

    def test_ripples_have_bounded_unit_normals(self):
        pixels = np.array(Image.open(io.BytesIO(normals()))).astype(float)
        n = pixels / 127.5 - 1
        np.testing.assert_allclose(np.linalg.norm(n,axis=-1), 1, atol=.007)
        self.assertGreater(n[:,:,2].min(), .93)
        self.assertLess(np.abs(n[:,0]-n[:,-1]).max(), .04)
        spectrum = sum(np.abs(np.fft.fft2(n[:,:,axis]))**2 for axis in (0, 1))
        spectrum[0, 0] = 0
        dominant = np.sort(spectrum.ravel())[-2:].sum() / spectrum.sum()
        self.assertLess(dominant, .20)  # no two diagonal waves dominate the pattern

    def test_shore_distance_uses_polygon_edges(self):
        triangles = np.array([[[0,0,0],[10,0,0],[10,10,0]],
                              [[0,0,0],[10,10,0],[0,10,0]]], dtype=float)
        points = np.array([[5,5,0],[1,5,0],[11,5,0]], dtype=float)
        np.testing.assert_allclose(shoreline_distance(triangles, points), [5,1,1])

    @unittest.skipUnless((ROOT / "origin_res/sn06/sn06.gmb").is_file(), "requires a local original game directory")
    def test_static_bake_excludes_water_smoke_and_clouds(self):
        for stage in ['sn06','sn09','sn13']:
            scene = read_scene(stage)
            _, _, _, ids, _ = static_triangles(scene)
            for i in np.unique(ids):
                self.assertTrue(scene[1][i][3].startswith('DISABLE'))
                self.assertEqual(scene[1][i][1], 'NORMAL')


if __name__ == '__main__':
    unittest.main()
