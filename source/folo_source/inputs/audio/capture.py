"""
The audio input's capture for this platform: WASAPI on Windows, the
PulseAudio client library on Linux (ADR-0031). Elsewhere the Windows module
imports without capture, lists no sources and reports why.
"""

import sys

if sys.platform == 'linux':
    from folo_source.inputs.audio.linux.audio_input import Capture, sources
else:
    from folo_source.inputs.audio.windows.audio_input import Capture, sources

__all__ = ['Capture', 'sources']
