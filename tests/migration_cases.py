"""
Sample v1 files for the migration goldens (RB-05 step 3): the default
configuration, saved presets, every other setting changed, a built-in look
with a built-in default, and a hand-edited file with missing and repeated
ids. tests/golden/migration/<name>/ holds each input and the documents it
migrates to (scripts/make_golden.py --only migration).
"""

from __future__ import annotations

import json
from pathlib import Path
from typing import Any

from folo_source.legacy import migrate, v1
from tests import golden_cases

MIGRATION_DIR = golden_cases.GOLDEN_DIR / 'migration'
INPUT = 'studio.json'
OUTPUT = 'out'


def fixed(config: dict[str, Any], prefix: str = 'seg') -> dict[str, Any]:
    """default_config() makes random segment ids: the samples' are fixed."""
    for number, segment in enumerate(config['segments'], start=1):
        segment['id'] = f'{prefix}{number}'
    return config


def default() -> dict[str, Any]:
    return fixed(v1.sanitize_config(v1.default_config(150)))


def presets() -> dict[str, Any]:
    config = fixed(v1.sanitize_config(v1.default_config(300)))
    shorter = fixed(v1.sanitize_config(v1.default_config(120)), 'short')
    config['presets'] = [
        {
            'id': 'chill1',
            'name': 'Chill evening',
            'desc': 'Slow and blue',
            'tags': ['Chill', 'Ambient'],
            'led_count': 120,
            'segments': shorter['segments'],
        },
        {
            'id': 'party',
            'name': 'Party',
            'desc': '',
            'tags': [],
            'led_count': 300,
            'segments': config['segments'][:2],
        },
    ]
    config['default_preset'] = 'party'
    return v1.sanitize_config(config)


def settings() -> dict[str, Any]:
    config = fixed(v1.default_config(90))
    config.update(
        {
            'audio': {
                'device': 'in:Line In',
                'app': 'spotify.exe',
                'app_mode': 'except',
                'gain': -6.5,
                'auto_level': True,
            },
            'hyperion': {
                'enabled': False,
                'host': 'hyperion.local',
                'port': 19445,
                'priority': 50,
                'fps': 60,
            },
            'master': 0.8,
            'background': '#000080',
            'reactive': 'auto',
            'ambient': {'color': '#FF3399', 'style': 'breathe', 'level': 0.5, 'idle_after': 30},
            'beat': {'offset': -40},
            'now_playing': {'enabled': False, 'cover_colors': True, 'pause_ambient': False},
            'default_preset': 'gone',
        }
    )
    return v1.sanitize_config(config)


def builtin() -> dict[str, Any]:
    case = next(c for c in golden_cases.builtin_cases() if c.builtin == ('genre:edm', 144))
    config = dict(case.config)
    config['default_preset'] = 'genre:edm'
    return v1.sanitize_config(config)


def repairs() -> dict[str, Any]:
    """Not sanitized: what a hand-edited file may look like."""
    return {
        'led_count': '200',
        'segments': [
            {'name': 'Lows', 'start': 0, 'end': 99, 'freq_lo': 30, 'freq_hi': 150},
            {'id': 'twin', 'start': 100, 'end': 199, 'trigger': 'hits'},
            {'id': 'twin', 'start': 50, 'end': 60, 'mode': 'nonsense'},
        ],
        'presets': [{'name': 'No id', 'segments': [{'start': 0, 'end': 10}]}],
        'hyperion': {'fps': 1000},
    }


SAMPLES = {
    'default': default,
    'presets': presets,
    'settings': settings,
    'builtin': builtin,
    'repairs': repairs,
}


def input_bytes(name: str) -> bytes:
    return (json.dumps(SAMPLES[name](), indent=2) + '\n').encode('utf-8')


def expected(name: str) -> dict[Path, bytes]:
    """The committed documents of a sample, by their path in the output folder."""
    out = MIGRATION_DIR / name / OUTPUT
    return {path.relative_to(out): path.read_bytes() for path in out.rglob('*.json')}


def write(name: str) -> None:
    """Writes a sample's input and the documents it migrates to."""
    folder = MIGRATION_DIR / name
    data = input_bytes(name)
    folder.mkdir(parents=True, exist_ok=True)
    (folder / INPUT).write_bytes(data)
    out = folder / OUTPUT
    for path in sorted(out.rglob('*.json'), reverse=True):
        path.unlink()
    for relative, document in migrate.migrate(migrate.read_v1(data)).items():
        path = out / relative
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_bytes(migrate.dumps(document))
