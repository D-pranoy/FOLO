"""
Album art, whatever the platform reads it from: its size limits and the
main colours picked for the LEDs (cover_colors).
"""

import numpy as np

MAX_ART_BYTES = 4 * 1024 * 1024
ART_PIXELS = 48  # the art is scaled to this square to pick its colors
MAX_COLORS = 5
HUE_BINS = 36
MIN_SEPARATION = 3  # bins (30 degrees) between the hues picked
MIN_SHARE = 0.08  # a hue needs this share of the strongest one's weight
MIN_COLORFUL = 0.04  # share of colorful pixels below which art counts as grey


def cover_colors(pixels):
    """
    The main colors of an image (rows x cols x rgb, or pixels x rgb, 0-255)
    as hex, the strongest first, at full brightness for the leds. Vivid
    colors count more than large dull areas. Empty for grey or dark art.
    """
    rgb = np.asarray(pixels, dtype=np.float64).reshape(-1, 3) / 255.0
    top = rgb.max(axis=1)
    chroma = top - rgb.min(axis=1)
    keep = (top > 0.12) & (chroma > 0.12)
    if keep.mean() < MIN_COLORFUL:
        return ()
    rgb, top, chroma = rgb[keep], top[keep], chroma[keep]
    r, g, b = rgb.T
    hue = (
        np.where(
            top == r,
            ((g - b) / chroma) % 6.0,
            np.where(top == g, (b - r) / chroma + 2.0, (r - g) / chroma + 4.0),
        )
        / 6.0
    )
    bins = (hue * HUE_BINS).astype(int) % HUE_BINS
    weight = chroma**2
    hist = np.bincount(bins, weights=weight, minlength=HUE_BINS)
    hist = 0.5 * hist + 0.25 * (np.roll(hist, 1) + np.roll(hist, -1))
    # Each pixel at full brightness: only its hue and saturation are left
    full = rgb / top[:, None]
    colors = []
    strongest = hist.max()
    while len(colors) < MAX_COLORS:
        peak = int(np.argmax(hist))
        if hist[peak] <= 0 or hist[peak] < strongest * MIN_SHARE:
            break
        near = np.abs((bins - peak + HUE_BINS // 2) % HUE_BINS - HUE_BINS // 2) <= 1
        color = (full[near] * weight[near, None]).sum(axis=0) / weight[near].sum()
        # Saturation up a little: on leds pale colors wash out to white
        low = color.min()
        boosted = 1.0 - (1.0 - color) * min(1.0, (1.0 - low) * 1.3) / max(1e-6, 1.0 - low)
        boosted /= boosted.max()
        colors.append('#{:02X}{:02X}{:02X}'.format(*[round(c * 255) for c in boosted]))
        for k in range(-MIN_SEPARATION + 1, MIN_SEPARATION):
            hist[(peak + k) % HUE_BINS] = 0.0
    return tuple(colors)
