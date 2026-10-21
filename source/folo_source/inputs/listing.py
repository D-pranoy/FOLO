"""
The inputs available on this source (RB-04 step 2), as the source link's
`inputs.list` returns them (api.json's InputList): every output, input and
app playing audio, the screens, and whether now playing can be read. Which
of them are enabled is in the source's input settings (sources.json).
"""

from __future__ import annotations

from typing import Any

from folo_source.inputs.audio import capture


def list_inputs(media: bool) -> dict[str, Any]:
    """`media`: whether the media input can read now playing here."""
    sources = capture.sources()
    devices = sources['devices']
    return {
        'default_output': sources['default'],
        'outputs': [{'name': d['name']} for d in devices if d['output']],
        'inputs': [{'name': d['name']} for d in devices if not d['output']],
        'apps': sources['apps'],
        'screens': [],  # listed by the capture agent (RB-14)
        'media': media,
    }
