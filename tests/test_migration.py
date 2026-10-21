"""
The v1 configuration migration (RB-05): the sample files migrate to their
golden documents, which pass the schemas; a second run changes nothing and
keeps the first backup; and every golden case, migrated and then rendered
through the effect engine's pipeline, still gives the golden frames.
"""

from __future__ import annotations

import json
import shutil
from pathlib import Path
from typing import Any, cast

import numpy as np
import pytest
from jsonschema import Draft202012Validator

from folo_source.cli import main
from folo_source.contracts import clamp
from folo_source.inputs.audio.analyzer import Analyzer
from folo_source.legacy import migrate
from folo_source.publish import frames
from folo_source.runtime.executor import Executor, Step
from tests import golden_cases as g
from tests import migration_cases
from tests.test_executor import STEP_US, Media
from tests.test_pipeline_parity import CASES, MANIFEST, ORDER

DEFINITIONS = {
    'zones.json': 'ZonesDocument',
    'show.json': 'ShowDocument',
    'providers.json': 'ProvidersDocument',
    'sources.json': 'SourcesDocument',
    migrate.MANIFEST: 'PackManifest',
}


def validator(definition: str) -> Draft202012Validator:
    bundle = json.loads(clamp.BUNDLE.read_text(encoding='utf-8'))
    return Draft202012Validator({'$ref': f'#/$defs/{definition}', '$defs': bundle['$defs']})


def run_tool(studio: Path, out: Path) -> dict[Path, bytes]:
    assert main(['migrate-v1', str(studio), str(out)]) == 0
    return {path.relative_to(out): path.read_bytes() for path in out.rglob('*.json')}


@pytest.mark.parametrize('name', list(migration_cases.SAMPLES))
def test_samples_migrate_to_their_goldens(name: str, tmp_path: Path) -> None:
    folder = migration_cases.MIGRATION_DIR / name
    assert (folder / migration_cases.INPUT).read_bytes() == migration_cases.input_bytes(name)
    studio = tmp_path / 'studio.json'
    shutil.copyfile(folder / migration_cases.INPUT, studio)
    written = run_tool(studio, tmp_path / 'out')
    assert written == migration_cases.expected(name)
    for path, data in written.items():
        definition = DEFINITIONS.get(path.name, 'SceneItem')
        errors = list(validator(definition).iter_errors(json.loads(data)))  # pyright: ignore[reportUnknownMemberType]
        assert not errors, f'{path}: {errors[0].message}'


def test_a_second_run_changes_nothing(tmp_path: Path) -> None:
    studio = tmp_path / 'studio.json'
    studio.write_bytes(migration_cases.input_bytes('repairs'))
    first = run_tool(studio, tmp_path / 'out')
    backup = studio.with_name('studio.json.v1.bak')
    assert backup.read_bytes() == studio.read_bytes()
    studio.write_bytes(migration_cases.input_bytes('default'))  # edited since
    run_tool(studio, tmp_path / 'out')
    assert backup.read_bytes() == migration_cases.input_bytes('repairs')  # the first stays
    studio.write_bytes(migration_cases.input_bytes('repairs'))
    assert run_tool(studio, tmp_path / 'out') == first


def test_presets_that_are_gone_leave_the_pack(tmp_path: Path) -> None:
    studio = tmp_path / 'studio.json'
    studio.write_bytes(migration_cases.input_bytes('presets'))
    run_tool(studio, tmp_path / 'out')
    studio.write_bytes(migration_cases.input_bytes('default'))
    written = run_tool(studio, tmp_path / 'out')
    assert not [p for p in written if p.parent.name == 'scenes']


def test_a_broken_file_is_refused(tmp_path: Path) -> None:
    studio = tmp_path / 'studio.json'
    studio.write_text('{"segments": [', encoding='utf-8')
    assert main(['migrate-v1', str(studio), str(tmp_path / 'out')]) == 1
    assert main(['migrate-v1', str(tmp_path / 'missing.json'), str(tmp_path / 'out')]) == 1
    assert not (tmp_path / 'out').exists()


def test_default_scene() -> None:
    config: dict[str, Any] = {'presets': [{'id': 'mine'}], 'default_preset': 'mine'}
    assert migrate.default_scene(config) == 'mine'
    assert (
        migrate.default_scene(config | {'default_preset': 'fx:ledfx-energy'}) == 'fx:ledfx-energy'
    )
    assert migrate.default_scene(config | {'default_preset': 'gone'}) == ''


def config_apply(documents: migrate.Documents) -> dict[str, Any]:
    """What a node would send its local source for the migrated documents (M3 does this)."""
    zone = documents[Path('zones.json')]['zones'][0]
    show = documents[Path('show.json')]
    surface = zone['surfaces'][0]
    scene_id = show['active'][zone['id']]
    width = surface['resolution']['width']
    params = {
        'revision': 1,
        'inputs': documents[Path('sources.json')]['sources'][0]['inputs'],
        'surfaces': [
            {
                'index': 0,
                'surface': surface,
                'scene': scene_id,
                'role': surface['role'],
                'beat_offset_ms': zone['beat_offset_ms'],
                'album_colors': zone['album_colors'],
            }
        ],
        'scenes': [s for s in show['scenes'] if s['id'] == scene_id],
        'publish': {
            'surfaces': [
                {
                    'index': 0,
                    'width': width,
                    'height': 1,
                    'rate_hz': 200,
                    'pixel_format': 'rgb8',
                    'lz4': False,
                }
            ],
            'signals': [],
        },
    }
    return cast(dict[str, Any], clamp.clamp('ConfigApplyParams', params))


@pytest.mark.parametrize('name', ORDER)
def test_migrated_configs_render_the_goldens(name: str) -> None:
    case = CASES[name]
    config = dict(case.config, now_playing=dict(case.config['now_playing']))
    config['now_playing']['cover_colors'] = bool(case.colors)
    data = json.dumps(config).encode('utf-8')
    documents = migrate.migrate(migrate.read_v1(data))
    sent: list[bytes] = []
    executor = Executor(
        Analyzer(), np.random.default_rng(g.SEED), Media(tuple(case.colors)), sent.append
    )
    executor.apply(config_apply(documents))
    t = 0
    for packet in g.analyse(case.analysis):
        t += STEP_US
        executor.step([Step(packet.spectrum, packet.frames, packet.dt)], t)
    for dt in g.idle_steps(case.idle):
        t += STEP_US
        executor.step([Step.silence(dt)], t)
    actual = np.stack([frames.decode_surface_frame(data).pixels[0] for data in sent])
    expected = g.read_png(g.RENDERER_DIR / MANIFEST['cases'][name]['png'])
    assert actual.shape == expected.shape
    differ = np.flatnonzero((actual != expected).any(axis=(1, 2)))
    assert not differ.size, f'{name}: frames differ from frame {int(differ[0])}'
