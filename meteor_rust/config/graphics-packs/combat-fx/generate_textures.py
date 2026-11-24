#!/usr/bin/env python3
"""Procedural placeholder textures for the combat-fx sample pack.

Run from anywhere: python3 meteor_rust/config/graphics-packs/combat-fx/generate_textures.py
Outputs PNGs next to this script. Replace them with authored art for release.
"""
import os
import numpy as np
from PIL import Image

HERE = os.path.dirname(os.path.abspath(__file__))


def save(name, rgba):
    rgba = np.clip(rgba * 255.0 + 0.5, 0, 255).astype(np.uint8)
    Image.fromarray(rgba, "RGBA").save(os.path.join(HERE, name), optimize=True)
    print("wrote", name, rgba.shape)


def value_noise(h, w, cells_y, cells_x, seed):
    rng = np.random.default_rng(seed)
    grid = rng.random((cells_y + 1, cells_x + 1))
    ys = np.linspace(0, cells_y, h, endpoint=False)
    xs = np.linspace(0, cells_x, w, endpoint=False)
    y0 = np.floor(ys).astype(int)
    x0 = np.floor(xs).astype(int)
    fy = (ys - y0)[:, None]
    fx = (xs - x0)[None, :]
    fy = fy * fy * (3 - 2 * fy)
    fx = fx * fx * (3 - 2 * fx)
    a = grid[y0][:, x0]
    b = grid[y0][:, x0 + 1]
    c = grid[y0 + 1][:, x0]
    d = grid[y0 + 1][:, x0 + 1]
    return (a * (1 - fx) + b * fx) * (1 - fy) + (c * (1 - fx) + d * fx) * fy


def trail_soft(w=256, h=64):
    """u (x) runs along the trail, v (y) from base to tip. Streaks fade with age."""
    u = np.linspace(0, 1, w)[None, :]
    v = np.linspace(0, 1, h)[:, None]
    streaks = 0.55 + 0.45 * value_noise(h, w, 6, 24, 7)
    streaks = streaks * (0.6 + 0.4 * value_noise(h, w, 3, 64, 11))
    edge = np.clip(np.sin(np.pi * v), 0, 1) ** 0.5
    body = streaks * edge
    alpha = np.clip(body * (1.0 - 0.25 * u), 0, 1)
    rgb = np.ones((h, w, 3))
    # Slight cool tint towards the tail so the pose colour dominates the head.
    rgb[..., 2] = 1.0 - 0.15 * u
    return np.dstack([rgb, alpha])


def spark(w=64, h=64):
    y, x = np.mgrid[0:h, 0:w]
    cx, cy = (w - 1) / 2, (h - 1) / 2
    r = np.sqrt(((x - cx) / cx) ** 2 + ((y - cy) / cy) ** 2)
    core = np.clip(1.0 - r / 0.25, 0, 1) ** 1.5
    glow = np.exp(-(r / 0.55) ** 2)
    alpha = np.clip(glow * 0.8 + core, 0, 1)
    rgb = np.ones((h, w, 3))
    rgb[..., 1] = 0.85 + 0.15 * core
    rgb[..., 2] = 0.6 + 0.4 * core
    return np.dstack([rgb, alpha])


def dust(w=128, h=128):
    y, x = np.mgrid[0:h, 0:w]
    cx, cy = (w - 1) / 2, (h - 1) / 2
    r = np.sqrt(((x - cx) / cx) ** 2 + ((y - cy) / cy) ** 2)
    puff = np.clip(1.0 - r, 0, 1) ** 1.8
    puff *= 0.6 + 0.4 * value_noise(h, w, 5, 5, 3)
    puff *= 0.75 + 0.25 * value_noise(h, w, 12, 12, 5)
    rgb = np.ones((h, w, 3)) * np.array([0.72, 0.66, 0.58])
    return np.dstack([rgb, np.clip(puff, 0, 1)])


def impact_flash(frames_x=4, frames_y=4, cell=64):
    """Expanding ring + fading core, 16 frames row-major."""
    sheet = np.zeros((frames_y * cell, frames_x * cell, 4))
    total = frames_x * frames_y
    y, x = np.mgrid[0:cell, 0:cell]
    c = (cell - 1) / 2
    r = np.sqrt(((x - c) / c) ** 2 + ((y - c) / c) ** 2)
    ang = np.arctan2(y - c, x - c)
    for i in range(total):
        t = i / (total - 1)
        radius = 0.15 + 0.8 * t ** 0.7
        width = 0.10 + 0.08 * t
        ring = np.exp(-((r - radius) / width) ** 2)
        # Spiky core that fades in the first third.
        spikes = 0.5 + 0.5 * np.cos(ang * 6.0 + t * 3.0)
        core = np.clip(1.0 - r / (0.35 * (1 - t) + 0.02), 0, 1) ** 1.2 * (0.5 + 0.5 * spikes)
        fade = (1.0 - t) ** 1.2
        alpha = np.clip((ring * 0.9 + core) * fade, 0, 1)
        rgb = np.ones((cell, cell, 3))
        rgb[..., 1] = 0.75 + 0.25 * core
        rgb[..., 2] = 0.45 + 0.55 * core
        fx, fy = i % frames_x, i // frames_x
        sheet[fy * cell:(fy + 1) * cell, fx * cell:(fx + 1) * cell] = np.dstack([rgb, alpha])
    return sheet


if __name__ == "__main__":
    save("trail-soft.png", trail_soft())
    save("spark.png", spark())
    save("dust.png", dust())
    save("impact-flash-4x4.png", impact_flash())
