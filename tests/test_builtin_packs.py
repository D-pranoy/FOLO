"""
The built-in packs are the studio's built-ins as data (RB-09 code change 2,
ADR-0038 D17 to D20): their shape, and their equivalence to the builders of
web/studio.js: the golden frames byte for byte, the layers at every required
length, the reactions and the palettes.

The tests that run Node (scripts/export_builtins.mjs) are skipped without it,
or fail without it when FOLO_NODE_TESTS=1, which CI's contracts job sets.
"""

from __future__ import annotations

import hashlib
import json
import os
import shutil
import subprocess
from functools import cache
from pathlib import Path
from typing import Any, cast

import numpy as np
import pytest

from folo_source.contracts import clamp
from folo_source.legacy import v1
from folo_source.legacy.renderer import PARAMS, layers_from_v1
from folo_source.surfaces.renderer import strip_cells
from tests import golden_cases as g

ROOT = Path(__file__).resolve().parent.parent
PACKS = ROOT / 'packs'
FOLO = PACKS / 'folo'
EXPORTER = ROOT / 'scripts' / 'export_builtins.mjs'
ROLE = 'strip.main'
# ADR-0038 D20: every length from 1 to 64, and these
LENGTHS = [*range(1, 65), 100, 144, 255, 256, 300, 512, 1000, 1024, 1999, 2000]
FRAME_LENGTHS = (30, 61, 144)
PREFIXES = {'fx:': 'folo/effects', 'genre:': 'folo/genres'}
NODE_TESTS = os.environ.get('FOLO_NODE_TESTS') == '1'

Json = Any
needs_node = pytest.mark.skipif(
    not NODE_TESTS and shutil.which('node') is None, reason='needs Node.js (FOLO_NODE_TESTS=1)'
)


def read(path: Path) -> Json:
    return json.loads(path.read_text(encoding='utf-8'))


def pack_dir(pack: str) -> Path:
    return PACKS / pack


def items(pack: str, kind: str) -> dict[str, Json]:
    """The items of one folder of a pack, by id."""
    folder = pack_dir(pack) / kind
    found: dict[str, Json] = {}
    for path in sorted(folder.glob('*.json')):
        document = read(path)
        found[document['id']] = document
    return found


def scenes() -> dict[str, Json]:
    """Every built-in scene by reference (`folo/genres:edm`)."""
    return {
        f'{pack}:{item_id}': scene
        for pack in ('folo/core', 'folo/genres', 'folo/effects')
        for item_id, scene in items(pack, 'scenes').items()
    }


def reference(builtin_id: str) -> str:
    for prefix, pack in PREFIXES.items():
        if builtin_id.startswith(prefix):
            return f'{pack}:{builtin_id[len(prefix) :]}'
    raise ValueError(builtin_id)


def scene_layers(scene: Json) -> list[Json]:
    return cast(list[Json], scene['scene']['layers_by_role'][ROLE])


# ---------------------------------------------------------------- shape (no Node)


def test_the_packs_hold_what_adr_0038_d17_lists() -> None:
    assert len(items('folo/core', 'palettes')) == 9
    assert len(items('folo/core', 'reactions')) == 25
    assert list(items('folo/core', 'scenes')) == ['default']
    assert len(items('folo/genres', 'scenes')) == 14
    assert len(items('folo/effects', 'scenes')) == 32
    ledfx = [s for s in items('folo/effects', 'scenes').values() if 'LedFx' in s['scene']['tags']]
    assert len(ledfx) == 18


@pytest.mark.parametrize('pack', ['folo/core', 'folo/genres', 'folo/effects'])
def test_each_manifest_lists_its_files_and_contents(pack: str) -> None:
    folder = pack_dir(pack)
    manifest = read(folder / 'folo-pack.json')
    assert clamp.clamp('PackManifest', manifest) == manifest
    assert manifest['id'] == pack
    assert manifest['version'] == '1.0.0'
    assert manifest['license'] == 'MIT'
    assert manifest['authors'] == ['FOLO contributors']
    on_disk = sorted(
        p.relative_to(folder).as_posix()
        for p in folder.rglob('*.json')
        if p.name != 'folo-pack.json'
    )
    assert sorted(manifest['files']) == on_disk
    for path, sha in manifest['files'].items():
        assert hashlib.sha256((folder / path).read_bytes()).hexdigest() == sha, path
    listed = sorted(
        f'{kind}/{item_id}.json' for kind, ids in manifest['contents'].items() for item_id in ids
    )
    assert listed == on_disk


@pytest.mark.parametrize(
    ('definition', 'pack', 'kind'),
    [
        ('PaletteItem', 'folo/core', 'palettes'),
        ('ReactionItem', 'folo/core', 'reactions'),
        ('SceneItem', 'folo/core', 'scenes'),
        ('SceneItem', 'folo/genres', 'scenes'),
        ('SceneItem', 'folo/effects', 'scenes'),
    ],
)
def test_items_are_clamped_already(definition: str, pack: str, kind: str) -> None:
    for item_id, document in items(pack, kind).items():
        assert clamp.clamp(definition, document) == document, item_id


def test_scenes_have_one_role_and_nearest_relative_layers() -> None:
    for ref, scene in scenes().items():
        assert scene['led_count'] == 0, ref
        assert scene['description'], ref
        assert list(scene['scene']['layers_by_role']) == [ROLE], ref
        layers = scene_layers(scene)
        assert [layer['id'] for layer in layers] == [str(i) for i in range(1, len(layers) + 1)]
        for layer in layers:
            assert layer['effect']['id'] == 'audio-visualizer'
            assert layer['region']['kind'] == 'relative', ref
            assert layer['region']['snap'] == 'nearest', ref


def test_ledfx_inspired_descriptions_and_hints() -> None:
    for ref, scene in items('folo/effects', 'scenes').items():
        if 'LedFx' in scene['scene']['tags']:
            assert scene['description'].startswith('LedFx-inspired. '), ref
            assert scene['hints']['source'] == 'LedFx', ref
        else:
            assert 'source' not in scene['hints'], ref


def test_categories_reference_existing_scenes() -> None:
    manifest = read(FOLO / 'core' / 'folo-pack.json')
    known = set(scenes())
    categories = manifest['categories']
    assert [c['id'] for c in categories] == [
        'mine',
        'ledfx',
        'electronic',
        'hiphop',
        'poprock',
        'jazzclassical',
        'chill',
        'visualizers',
    ]
    assert categories[0]['items'] == []  # "mine" is filled by rule
    for category in categories:
        assert set(category['items']) <= known, category['id']
    assert len(categories[1]['items']) == 18


def test_the_alias_table_names_every_builtin() -> None:
    aliases = read(PACKS / 'aliases.json')
    assert len(aliases) == 46
    known = set(scenes())
    for builtin_id, ref in aliases.items():
        assert reference(builtin_id) == ref
        assert ref in known, builtin_id
    assert sum(k.startswith('genre:') for k in aliases) == 14


def test_the_default_scene_is_the_reset_segments_in_thirds() -> None:
    layers = scene_layers(items('folo/core', 'scenes')['default'])
    assert [layer['name'] for layer in layers] == ['Bass', 'Mids', 'Highs']
    segments = v1.default_config(30)['segments']
    for layer, segment in zip(layers, segments, strict=True):
        assert layer['params'] == {key: segment[key] for key in PARAMS}
    third = [strip_cells(layer['region'], 32) for layer in layers]
    assert third == [(0, 11), (11, 21), (21, 32)]  # ADR-0038 D20: 0-10, 11-20, 21-31
    for n in range(3, 400, 3):  # a multiple of 3: today's reset segments exactly
        expected = [(s['start'], s['end'] + 1) for s in v1.default_config(n)['segments']]
        assert [strip_cells(layer['region'], n) for layer in layers] == expected, n


# ---------------------------------------------------------------- frames


def builtin_golden_names() -> list[str]:
    return [
        c.name for c in g.builtin_cases() if c.builtin is not None and c.builtin[1] in FRAME_LENGTHS
    ]


@pytest.mark.parametrize('name', sorted(builtin_golden_names()))
def test_scene_frames_equal_the_golden(name: str, monkeypatch: pytest.MonkeyPatch) -> None:
    """The scene on a strip.main surface renders the builder's golden, byte for byte."""
    case = g.by_name(g.builtin_cases())[name]
    assert case.builtin is not None
    ref = reference(case.builtin[0])
    scene = scenes()[ref]
    used: list[int] = []

    def from_scene(config: dict[str, Any]) -> list[Json]:
        used.append(1)
        layers = scene_layers(scene)
        assert len(layers) == len(config['segments'])
        return [
            {**layer, 'id': seg['id']}
            for layer, seg in zip(layers, config['segments'], strict=True)
        ]

    monkeypatch.setattr('folo_source.legacy.renderer.layers_from_v1', from_scene)
    result = g.render(case)
    assert used, 'the scene was not what rendered'
    expected = g.read_png(g.RENDERER_DIR / f'{name}.png')
    assert np.array_equal(result.frames, expected), name


# ---------------------------------------------------------------- the builders (Node)


@cache
def builders() -> Json:
    """The builders' segments at every required length, from web/studio.js run in Node."""
    node = shutil.which('node')
    assert node is not None, 'FOLO_NODE_TESTS=1 needs Node.js'
    counts = ','.join(str(n) for n in LENGTHS)
    out = subprocess.run(
        [node, str(EXPORTER), '--led-counts', counts],
        check=True,
        capture_output=True,
        text=True,
        encoding='utf-8',
    ).stdout
    return json.loads(out)


@cache
def traced() -> Json:
    node = shutil.which('node')
    assert node is not None, 'FOLO_NODE_TESTS=1 needs Node.js'
    out = subprocess.run(
        [node, str(EXPORTER), '--trace'],
        check=True,
        capture_output=True,
        text=True,
        encoding='utf-8',
    ).stdout
    return json.loads(out)


def comparable(layer: Json) -> Json:
    """A layer without its id; its region is compared as cells."""
    return {key: value for key, value in layer.items() if key not in ('id', 'region')}


def builder_layers(segments: list[Json], count: int) -> list[Json]:
    """The builder's segments as the page posts them: sanitised, then as the migration's layers."""
    config = v1.sanitize_config({'segments': segments, 'led_count': count}, count)
    return layers_from_v1(config)


def builtin_ids() -> list[str]:
    return [b['id'] for b in read(g.BUILTINS)['builtins']]


@needs_node
@pytest.mark.parametrize('builtin_id', builtin_ids())
def test_layers_equal_the_builders_at_every_length(builtin_id: str) -> None:
    scene = scenes()[reference(builtin_id)]
    layers = scene_layers(scene)
    look = next(b for b in builders()['builtins'] if b['id'] == builtin_id)
    assert look['name'] == scene['scene']['name']
    for count in LENGTHS:
        expected = builder_layers(look['segments'][str(count)], count)
        assert len(layers) == len(expected), (builtin_id, count)
        for layer, wanted in zip(layers, expected, strict=True):
            first, stop = strip_cells(layer['region'], count)
            cells = {'kind': 'cells', 'start': first, 'end': stop - 1}
            assert cells == wanted['region'], (builtin_id, count, layer['name'])
            assert comparable(layer) == comparable(wanted), (builtin_id, count, layer['name'])


@needs_node
def test_reactions_and_palettes_equal_the_builders() -> None:
    data = traced()
    palettes = items('folo/core', 'palettes')
    assert sorted(p['name'] for p in palettes.values()) == sorted(
        p['name'] for p in data['palettes']
    )
    for source in data['palettes']:
        made = next(p for p in palettes.values() if p['name'] == source['name'])
        assert made['colors'] == source['colors']
    reactions = items('folo/core', 'reactions')
    assert sorted(r['name'] for r in reactions.values()) == sorted(
        r['name'] for r in data['reactions']
    )
    for source in data['reactions']:
        made = next(r for r in reactions.values() if r['name'] == source['name'])
        assert made['params'] == {**source['range'], **source['response']}
        assert made['hints']['group'] == source['group']
        assert made['description'] == source['desc']


@needs_node
def test_every_builder_has_a_scene_and_a_golden() -> None:
    ids = {b['id'] for b in traced()['builtins']}
    assert ids == set(builtin_ids()), 'tests/golden/builtins.json is out of date with web/studio.js'
    assert {reference(i) for i in ids} <= set(scenes())


@needs_node
def test_reaction_hints_and_categories_equal_the_builders() -> None:
    data = traced()
    reactions = items('folo/core', 'reactions')
    for source in data['reactions']:
        made = reactions[next(i for i, r in reactions.items() if r['name'] == source['name'])]
        assert made['hints'] == {k: source[k] for k in ('group', 'tempo', 'anim', 'dur')}
    categories = read(FOLO / 'core' / 'folo-pack.json')['categories']
    assert categories == [
        {
            'id': c['id'],
            'name': c['name'],
            'description': c['blurb'],
            'items': [reference(item) for item in c['items']],
        }
        for c in data['categories']
    ]


@needs_node
def test_scene_names_descriptions_and_tags_equal_the_builders() -> None:
    for look in traced()['builtins']:
        scene = scenes()[reference(look['id'])]
        assert scene['scene']['name'] == look['name']
        assert scene['scene']['tags'] == look['tags']
        assert scene['description'].endswith(look['desc'])
        assert scene['hints']['tempo'] == look['tempo']
