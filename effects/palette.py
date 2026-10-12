"""Helpers for effects that use a custom color palette."""

import webcolors


def parse_palette(palette):
    """
    Converts a list of color names or '#RRGGBB' strings to (r, g, b) floats
    in range 0-1. Returns None for an empty palette.
    """
    if not palette:
        return None
    colors = []
    for color in palette:
        if color.startswith('#'):
            rgb = webcolors.hex_to_rgb(color)
        else:
            rgb = webcolors.name_to_rgb(color)
        colors.append((rgb[0] / 255.0, rgb[1] / 255.0, rgb[2] / 255.0))
    return colors


def palette_color(colors, s):
    """
    Returns the color at position s (0.0-1.0) of a smooth gradient running
    through all the palette colors.
    """
    if len(colors) == 1:
        return colors[0]
    s = max(0.0, min(s, 1.0))
    pos = s * (len(colors) - 1)
    i = min(int(pos), len(colors) - 2)
    t = pos - i
    a = colors[i]
    b = colors[i + 1]
    return (a[0] + (b[0] - a[0]) * t,
            a[1] + (b[1] - a[1]) * t,
            a[2] + (b[2] - a[2]) * t)
