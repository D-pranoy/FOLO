"""
The built-in packs as data (RB-09, ADR-0038 D17 to D20): `packs/folo/core`,
`packs/folo/genres` and `packs/folo/effects`, and the alias table
`packs/aliases.json`, generated from the builders of web/studio.js while it
exists (scripts/export_builtins.mjs --trace).

    uv run python scripts/build_packs.py generate   # rewrite packs/
    uv run python scripts/build_packs.py check      # fail when packs/ is out of date

Needs Node.js. Generated files are committed, never edited; CI runs `check`.
When the UI's switch-over removes web/ (RB-11), packs/folo/ becomes the
source and this script goes.
"""

from __future__ import annotations

import argparse
import difflib
import hashlib
import json
import re
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path
from typing import Any

ROOT = Path(__file__).resolve().parent.parent
sys.path.insert(0, str(ROOT / 'source'))

from folo_source.contracts import clamp  # noqa: E402
from folo_source.legacy import v1  # noqa: E402
from folo_source.legacy.renderer import PARAMS  # noqa: E402

PACKS = ROOT / 'packs'
ALIASES = 'aliases.json'
EXPORTER = ROOT / 'scripts' / 'export_builtins.mjs'
MANIFEST = 'folo-pack.json'
ROLE = 'strip.main'
EFFECT = {'id': 'audio-visualizer', 'version': '1.0.0'}
VERSION = '1.0.0'
LICENSE = 'MIT'
AUTHORS = ['FOLO contributors']
COMPATIBLE = '>=1.0 <2'
PREFIXES = {'fx:': 'folo/effects', 'genre:': 'folo/genres'}
LEDFX_NOTE = 'LedFx-inspired.'
DEFAULT_SCENE_DESCRIPTION = 'Bass, mids and highs over thirds of the strip.'
DEFAULT_SCENE_TAGS: list[str] = []
FOLDERS = {
    'palettes': 'palettes',
    'reactions': 'reactions',
    'scenes': 'scenes',
}
PACK_TEXT = {
    'folo/core': (
        'FOLO core',
        'The palettes, reactions and default scene every FOLO setup starts from.',
        ['core'],
    ),
    'folo/genres': ('FOLO genres', 'Scenes tuned for music genres.', ['genres']),
    'folo/effects': (
        'FOLO effects',
        'Whole-strip effects, including the LedFx-inspired ones.',
        ['effects'],
    ),
}

Json = Any


def slug(text: str) -> str:
    """The id of an item from its name, as the studio's slug() makes it."""
    return re.sub(r'[^a-z0-9]+', '-', text.lower()).strip('-')


def dumps(document: Json) -> bytes:
    """How every file is written: indented UTF-8 JSON with a final newline."""
    return (json.dumps(document, indent=2, ensure_ascii=False) + '\n').encode('utf-8')


def run_trace() -> Json:
    """The exporter's trace of the studio's built-ins (it checks the fractions itself)."""
    node = shutil.which('node')
    if node is None:
        raise SystemExit('build_packs.py needs Node.js (it runs scripts/export_builtins.mjs)')
    out = subprocess.run(
        [node, str(EXPORTER), '--trace'],
        check=True,
        capture_output=True,
        text=True,
        encoding='utf-8',
    ).stdout
    return json.loads(out)


def reference(item_id: str) -> str:
    """`author/pack:item` of a built-in id (`fx:beat-sync`, `genre:edm`)."""
    for prefix, pack in PREFIXES.items():
        if item_id.startswith(prefix):
            return f'{pack}:{item_id[len(prefix) :]}'
    raise ValueError(f'not a built-in id: {item_id}')


def layer(index: int, fields: Json) -> Json:
    """A scene layer from a traced segment: its fractions become a nearest-snapped region."""
    return {
        'id': str(index),
        'name': fields['name'],
        'effect': dict(EFFECT),
        'params': {key: fields[key] for key in PARAMS},
        'region': {
            'kind': 'relative',
            'start': fields['from'],
            'end': fields['to'],
            'snap': 'nearest',
        },
        'blend': 'max',
        'axis': 'linear_x',
        'enabled': fields['enabled'],
        'solo': fields['solo'],
        'lane': -1,
    }


def scene_item(
    item_id: str, name: str, description: str, tags: list[str], hints: Json, layers: list[Json]
) -> Json:
    """A scene item of one role, `strip.main`, with a layer per builder segment."""
    item: Json = {
        'id': item_id,
        'schema_version': 2,
        'description': description,
        'hints': hints,
        'scene': {
            'id': item_id,
            'name': name,
            'tags': tags,
            'layers_by_role': {ROLE: [layer(i, seg) for i, seg in enumerate(layers, start=1)]},
        },
        'led_count': 0,
    }
    return clamp.clamp('SceneItem', item)


def look_scene(look: Json) -> Json:
    """A built-in effect or genre preset as a scene item."""
    description = look['desc']
    hints: Json = {'tempo': look['tempo']}
    if look['source']:
        description = f'{LEDFX_NOTE} {description}'
        hints['source'] = look['source']
    item_id = reference(look['id']).split(':', 1)[1]
    return scene_item(item_id, look['name'], description, look['tags'], hints, look['layers'])


def default_scene() -> Json:
    """Today's reset segments, as `v1.default_config` makes them, over thirds."""
    segments = v1.default_config(30)['segments']
    edges = [0.0, 1 / 3, 2 / 3, 1.0]
    layers = [{**seg, 'from': edges[i], 'to': edges[i + 1]} for i, seg in enumerate(segments)]
    return scene_item(
        'default', 'Default', DEFAULT_SCENE_DESCRIPTION, DEFAULT_SCENE_TAGS, {}, layers
    )


def palette_item(palette: Json) -> Json:
    return clamp.clamp(
        'PaletteItem',
        {
            'id': slug(palette['name']),
            'schema_version': 2,
            'name': palette['name'],
            'colors': palette['colors'],
        },
    )


def reaction_item(reaction: Json) -> Json:
    hints = {key: reaction[key] for key in ('group', 'tempo', 'anim', 'dur')}
    return clamp.clamp(
        'ReactionItem',
        {
            'id': slug(reaction['name']),
            'schema_version': 2,
            'description': reaction['desc'],
            'hints': hints,
            'name': reaction['name'],
            'params': {**reaction['range'], **reaction['response']},
        },
    )


def category(raw: Json) -> Json:
    return {
        'id': raw['id'],
        'name': raw['name'],
        'description': raw['blurb'],
        'items': [reference(item) for item in raw['items']],
    }


def manifest(pack_id: str, contents: Json, files: Json, categories: list[Json] | None) -> Json:
    name, description, tags = PACK_TEXT[pack_id]
    document: Json = {
        'id': pack_id,
        'version': VERSION,
        'folo': COMPATIBLE,
        'name': name,
        'description': description,
        'license': LICENSE,
        'authors': AUTHORS,
        'tags': tags,
        'contents': contents,
        'files': files,
    }
    if categories is not None:
        document['categories'] = categories
    return clamp.clamp('PackManifest', document)


def pack_files(
    pack_id: str, items: dict[str, list[Json]], categories: list[Json] | None
) -> dict[str, bytes]:
    """The files of one pack, by path below its folder, as bytes."""
    files: dict[str, bytes] = {}
    for kind, documents in items.items():
        for document in documents:
            files[f'{FOLDERS[kind]}/{document["id"]}.json'] = dumps(document)
    hashes = {path: hashlib.sha256(data).hexdigest() for path, data in sorted(files.items())}
    contents = {kind: [d['id'] for d in documents] for kind, documents in items.items()}
    files[MANIFEST] = dumps(manifest(pack_id, contents, hashes, categories))
    return files


def build(trace: Json) -> dict[str, bytes]:
    """Every generated file, by path below `packs/`."""
    looks = trace['builtins']
    genres = [look_scene(b) for b in looks if b['kind'] == 'genre']
    effects = [look_scene(b) for b in looks if b['kind'] != 'genre']
    core_items = {
        'palettes': [palette_item(p) for p in trace['palettes']],
        'reactions': [reaction_item(r) for r in trace['reactions']],
        'scenes': [default_scene()],
    }
    categories = [category(c) for c in trace['categories']]
    packs = {
        'folo/core': pack_files('folo/core', core_items, categories),
        'folo/genres': pack_files('folo/genres', {'scenes': genres}, None),
        'folo/effects': pack_files('folo/effects', {'scenes': effects}, None),
    }
    out = {
        f'{pack_id}/{path}': data
        for pack_id, files in packs.items()
        for path, data in files.items()
    }
    aliases = {b['id']: reference(b['id']) for b in looks}
    out[ALIASES] = dumps(dict(sorted(aliases.items())))
    return out


def tree(folder: Path) -> dict[str, bytes]:
    return {
        path.relative_to(folder).as_posix(): path.read_bytes()
        for path in sorted(folder.rglob('*'))
        if path.is_file()
    }


def write(files: dict[str, bytes], folder: Path) -> None:
    for path, data in files.items():
        target = folder / path
        target.parent.mkdir(parents=True, exist_ok=True)
        target.write_bytes(data)


def generate() -> int:
    """Rewrites `packs/` whole: anything else in it is removed, as `check` reports it."""
    files = build(run_trace())
    if PACKS.exists():
        shutil.rmtree(PACKS)
    write(files, PACKS)
    print(f'wrote {len(files)} files to {PACKS.relative_to(ROOT)}')
    return 0


def check() -> int:
    """Regenerates into a temporary folder and fails on any difference."""
    with tempfile.TemporaryDirectory() as temporary:
        write(build(run_trace()), Path(temporary))
        expected, actual = tree(Path(temporary)), tree(PACKS) if PACKS.exists() else {}
    problems: list[str] = []
    for path in sorted(set(expected) | set(actual)):
        if path not in actual:
            problems.append(f'missing: packs/{path}')
        elif path not in expected:
            problems.append(f'unexpected: packs/{path}')
        elif expected[path] != actual[path]:
            diff = difflib.unified_diff(
                actual[path].decode().splitlines(),
                expected[path].decode().splitlines(),
                'committed',
                'generated',
                lineterm='',
                n=1,
            )
            problems.append(f'differs: packs/{path}\n' + '\n'.join(list(diff)[:20]))
    if problems:
        print('\n'.join(problems))
        print('packs/ is out of date: run `uv run python scripts/build_packs.py generate`')
        return 1
    print('packs/ is up to date')
    return 0


def main() -> int:
    parser = argparse.ArgumentParser(description='The built-in packs as data (RB-09)')
    parser.add_argument('command', choices=['generate', 'check'])
    return {'generate': generate, 'check': check}[parser.parse_args().command]()


if __name__ == '__main__':
    sys.exit(main())
