"""
The ambient light (config `reactive` and `ambient`). It stays in the
engine's legacy path until RB-08 moves it to the controller.
"""

import math
from typing import Any

import numpy as np
import numpy.typing as npt

from folo_source.effects.colors import hex_to_rgb
from folo_source.inputs.audio.analyzer import QUIET_DB

# Ambient light (config 'reactive' and 'ambient'): what the leds show when
# audio reactiveness is off, or in auto mode after a stretch of silence
AMBIENT_FADE_IN = 1.5  # seconds to fade into the ambient light
AMBIENT_FADE_OUT = 0.15  # seconds back to the reactive look, so the first beat shows
BREATHE_PERIOD = 8.0  # seconds per breath of the breathe style
BREATHE_LOW = 0.3  # breathe style: dimmest point, as a share of the color
# With now_playing.pause_ambient, auto mode switches as soon as the player is
# paused and the audio has been quiet this long
PAUSE_QUIET = 0.3


class Ambient:
    """
    Crossfades between the reactive frame and the ambient light. `amount` is
    0 for the reactive look and 1 for the ambient light; `quiet` counts the
    seconds the audio has been silent.
    """

    def __init__(self):
        self.amount = 0.0
        self.quiet = 0.0
        self.phase = 0.0

    def wanted(self, config, paused=False):
        """
        1 for the ambient light, 0 for the reactive look. `paused`: the
        player says the music is paused (only counts with pause_ambient).
        """
        mode = config['reactive']
        if mode == 'off':
            return 1.0
        if mode == 'auto':
            idle = config['ambient']['idle_after']
            if paused and config['now_playing']['pause_ambient']:
                idle = min(idle, PAUSE_QUIET)
            if self.quiet >= idle:
                return 1.0
        return 0.0

    def update(self, config, spectrum, dt, paused=False):
        self.quiet = 0.0 if spectrum.loudest >= QUIET_DB else self.quiet + dt
        self.phase = (self.phase + dt / BREATHE_PERIOD) % 1.0
        want = self.wanted(config, paused)
        if want > self.amount:
            self.amount = min(want, self.amount + dt / AMBIENT_FADE_IN)
        elif want < self.amount:
            self.amount = max(want, self.amount - dt / AMBIENT_FADE_OUT)

    def color(self, config):
        ambient = config['ambient']
        rgb = np.array(hex_to_rgb(ambient['color']), dtype=np.float64) * ambient['level']
        if ambient['style'] == 'breathe':
            breath = 0.5 - 0.5 * math.cos(2.0 * math.pi * self.phase)
            rgb *= BREATHE_LOW + (1.0 - BREATHE_LOW) * breath
        return rgb

    def blend(
        self,
        out: npt.NDArray[np.float64],
        config: dict[str, Any],
        spectrum: Any,
        dt: float,
        paused: bool = False,
    ) -> npt.NDArray[np.float64]:
        """
        Mixes the ambient light into `out` (leds x rgb, before `master`).
        """
        self.update(config, spectrum, dt, paused)
        if self.amount <= 0.0:
            return out
        a = self.amount * self.amount * (3.0 - 2.0 * self.amount)  # eased
        return out * (1.0 - a) + self.color(config) * a
