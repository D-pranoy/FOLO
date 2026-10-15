"""
Palette colors: hex to RGB, palettes as arrays, and album art colors in
place of a segment's palette.
"""

import math

import numpy as np


def hex_to_rgb(color):
    return (int(color[1:3], 16), int(color[3:5], 16), int(color[5:7], 16))


_palettes = {}


def palette_colors(palette):
    """
    A palette's colors as a (colors x rgb) array, read-only and shared.
    """
    key = tuple(palette)
    colors = _palettes.get(key)
    if colors is None:
        if len(_palettes) > 256:
            _palettes.clear()
        colors = np.array([hex_to_rgb(c) for c in palette], dtype=np.float64)
        colors.flags.writeable = False
        _palettes[key] = colors
    return colors


def cover_palette(seg, colors):
    """
    A segment's palette in the album art's colors: as many colors as it had,
    starting from the strongest for lows, the second for mids and the third
    for highs (by the middle of its band, like the web UI's kinds).
    """
    center = math.sqrt(seg['freq_lo'] * seg['freq_hi'])
    first = 0 if center < 250.0 else 1 if center < 4000.0 else 2
    return [colors[(first + i) % len(colors)] for i in range(len(seg['palette']))]
