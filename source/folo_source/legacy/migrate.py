"""
The v1 configuration migration (RB-05,
docs/specs/config-model.md#migration-from-the-current-studiojson-version-1):
today's `studio.json` becomes the v2 documents. The same file always gives
the same documents, byte for byte, so the tool can run more than once: ids
are fixed, or derived from the file when sanitize_config() has to make them.

    folo-source migrate-v1 <studio.json> <out-dir>
"""

from __future__ import annotations

import hashlib
import itertools
import json
import logging
import os
import tempfile
from collections.abc import Generator, Mapping
from contextlib import contextmanager
from pathlib import Path
from typing import Any, cast

from folo_source.contracts import clamp
from folo_source.legacy import v1
from folo_source.legacy.renderer import layers_from_v1, surface_from_v1

log = logging.getLogger(__name__)

ZONE_ID = 'default'
SURFACE_ID = 'main'
ROLE = 'strip.main'
TARGET_ID = 'strip'
MAPPING_ID = 'main-strip'
PROVIDER_ID = 'hyperion'
SOURCE_ID = 'local'
SCENE_ID = 'current'
PACK_ID = 'local/my-presets'
PACK_DIR = Path('library') / 'local' / 'my-presets'
MANIFEST = 'folo-pack.json'
BACKUP_SUFFIX = '.v1.bak'
# Built-in looks live in the UI; their ids stay as they are for the alias table (RB-09)
BUILTIN_PREFIXES = ('fx:', 'genre:')

Json = Any
Documents = dict[Path, Json]  # path in the output folder: document


class MigrationError(Exception):
    """The file can't be read as a v1 configuration."""


@contextmanager
def _stable_ids(seed: bytes) -> Generator[None]:
    """
    sanitize_config() gives missing or repeated ids random new ones: for the
    migration they come from the file's contents instead, so a second run
    gives the same ids. The tool is single-threaded, so swapping the
    module's id maker for the run is safe.
    """
    counter = itertools.count(1)
    made = v1.new_id

    def stable() -> str:
        return hashlib.sha256(seed + str(next(counter)).encode('ascii')).hexdigest()[:8]

    v1.new_id = stable
    try:
        yield
    finally:
        v1.new_id = made


def read_v1(data: bytes) -> dict[str, Any]:
    """A studio.json's bytes as a sanitized v1 configuration."""
    try:
        raw = json.loads(data.decode('utf-8'))
    except (UnicodeDecodeError, ValueError) as error:
        raise MigrationError(f'not a JSON file: {error}') from error
    with _stable_ids(hashlib.sha256(data).digest()):
        return v1.sanitize_config(raw)


def _no_new_ids() -> str:
    raise AssertionError('every id of a migrated document is set')  # pragma: no cover


def _clamped(definition: str, document: Json) -> Json:
    return clamp.clamp(definition, document, new_id=_no_new_ids)


def migrate(config: Mapping[str, Any]) -> Documents:
    """
    The v2 documents of a sanitized v1 configuration: zones.json, show.json,
    providers.json, sources.json and the local "My presets" pack, each
    clamped like any document the node reads.
    """
    config = dict(config)
    presets = cast(list[dict[str, Any]], config['presets'])
    documents: Documents = {
        Path('zones.json'): _clamped('ZonesDocument', {'zones': [_zone(config)]}),
        Path('show.json'): _clamped('ShowDocument', _show(config)),
        Path('providers.json'): _clamped('ProvidersDocument', _providers(config)),
        Path('sources.json'): _clamped('SourcesDocument', _sources(config)),
    }
    documents.update(_pack(presets))
    return documents


def _zone(config: Mapping[str, Any]) -> dict[str, Any]:
    count = int(config['led_count'])
    surface = surface_from_v1(dict(config)) | {'id': SURFACE_ID, 'sources': [SOURCE_ID]}
    ambient = cast(Mapping[str, Any], config['ambient'])
    playing = cast(Mapping[str, Any], config['now_playing'])
    span = {'start': 0, 'count': count}
    return {
        'id': ZONE_ID,
        'name': 'Default',
        'type': 'custom',
        'targets': [
            {
                'id': TARGET_ID,
                'name': 'Hyperion strip',
                'kind': 'line',
                'provider': {'instance': PROVIDER_ID, 'address': span},
                'geometry': {'kind': 'line', 'count': count},
            }
        ],
        'surfaces': [surface],
        'mappings': [
            {
                'id': MAPPING_ID,
                'surface': SURFACE_ID,
                'target': TARGET_ID,
                'method': 'manual',
                'ranges': [{'cells': span, 'elements': span}],
            }
        ],
        'reactive': config['reactive'],
        'ambient': dict(ambient) | {'on_pause': playing['pause_ambient']},
        'beat_offset_ms': cast(Mapping[str, Any], config['beat'])['offset'],
        'album_colors': playing['cover_colors'],
    }


def _show(config: Mapping[str, Any]) -> dict[str, Any]:
    scene = {
        'id': SCENE_ID,
        'name': 'From the studio',
        'layers_by_role': {ROLE: layers_from_v1(dict(config))},
    }
    return {
        'active': {ZONE_ID: SCENE_ID},
        'default_scene': default_scene(config),
        'scenes': [scene],
    }


def default_scene(config: Mapping[str, Any]) -> str:
    """
    The default scene: a saved preset's id is its scene's id in the
    "My presets" pack, a built-in id stays for the alias table, and anything
    else is none.
    """
    chosen = str(config['default_preset'])
    saved = {str(p['id']) for p in cast(list[Mapping[str, Any]], config['presets'])}
    if chosen in saved or chosen.startswith(BUILTIN_PREFIXES):
        return chosen
    return ''


def _providers(config: Mapping[str, Any]) -> dict[str, Any]:
    hyperion = cast(Mapping[str, Any], config['hyperion'])
    return {
        'providers': [
            {
                'id': PROVIDER_ID,
                'kind': 'hyperion',
                'name': 'Hyperion',
                'enabled': hyperion['enabled'],
                'rate_hz': hyperion['fps'],
                'settings': {
                    'kind': 'hyperion',
                    'host': hyperion['host'],
                    'port': hyperion['port'],
                    'priority': hyperion['priority'],
                },
            }
        ]
    }


def _sources(config: Mapping[str, Any]) -> dict[str, Any]:
    playing = cast(Mapping[str, Any], config['now_playing'])
    return {
        'sources': [
            {
                'id': SOURCE_ID,
                'name': 'This computer',
                'role': 'owner',
                'surfaces': [SURFACE_ID],
                'inputs': {
                    'audio': dict(cast(Mapping[str, Any], config['audio'])),
                    'media': {'enabled': playing['enabled']},
                    'screens': [],
                },
            }
        ]
    }


def _preset_layers(preset: Mapping[str, Any]) -> list[dict[str, Any]]:
    """A saved preset's segments as layers, as for the configuration's own."""
    segments = cast(list[Mapping[str, Any]], preset['segments'])
    return layers_from_v1({'segments': segments})


def _pack(presets: list[dict[str, Any]]) -> Documents:
    """The "My presets" pack: a scene item per saved preset, and its manifest."""
    documents: Documents = {}
    ids: list[str] = []
    for number, preset in enumerate(presets, start=1):
        item = {
            'id': preset['id'],
            'scene': {
                'id': preset['id'],
                'name': preset['name'],
                'tags': preset['tags'],
                'layers_by_role': {ROLE: _preset_layers(preset)},
            },
            'led_count': preset['led_count'],
        }
        ids.append(str(preset['id']))
        documents[PACK_DIR / 'scenes' / f'preset-{number:03d}.json'] = _clamped('SceneItem', item)
    files = {
        path.relative_to(PACK_DIR).as_posix(): hashlib.sha256(dumps(doc)).hexdigest()
        for path, doc in documents.items()
    }
    manifest = {
        'id': PACK_ID,
        'version': '1.0.0',
        'name': 'My presets',
        'description': 'Presets saved in the FOLO studio',
        'contents': {'scenes': ids},
        'files': files,
    }
    documents[PACK_DIR / MANIFEST] = _clamped('PackManifest', manifest)
    return documents


def dumps(document: Json) -> bytes:
    """How every document is written: indented UTF-8 JSON with a final newline."""
    return (json.dumps(document, indent=2, ensure_ascii=False) + '\n').encode('utf-8')


def _write(path: Path, data: bytes) -> None:
    """Writes a file atomically: a temporary file, fsync, then replace."""
    path.parent.mkdir(parents=True, exist_ok=True)
    handle, temporary = tempfile.mkstemp(prefix=f'.{path.name}.', dir=path.parent)
    try:
        with os.fdopen(handle, 'wb') as out:
            out.write(data)
            out.flush()
            os.fsync(out.fileno())
        os.replace(temporary, path)
    except BaseException:
        Path(temporary).unlink(missing_ok=True)
        raise


def migrate_file(studio: Path, out_dir: Path) -> list[Path]:
    """
    Migrates `studio` into `out_dir` and returns the files written. The
    first run keeps a backup of `studio` beside it (studio.json.v1.bak);
    later runs leave that backup alone. Scene files of presets no longer
    there are removed from the pack.
    """
    try:
        data = studio.read_bytes()
    except OSError as error:
        raise MigrationError(f'cannot read {studio}: {error}') from error
    documents = migrate(read_v1(data))
    backup = studio.with_name(studio.name + BACKUP_SUFFIX)
    if not backup.exists():
        _write(backup, data)
        log.info('kept a backup in %s', backup)
    written: list[Path] = []
    for relative, document in documents.items():
        path = out_dir / relative
        _write(path, dumps(document))
        written.append(path)
    scenes = out_dir / PACK_DIR / 'scenes'
    for stale in sorted(set(scenes.glob('*.json')) - set(written)):
        stale.unlink()
    return written
