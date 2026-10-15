"""
The prose specifications and the machine-readable contracts agree (RB-02
step 6): the tables of docs/specs/config-model.md, client-api.md and
source-link.md against specs/schema, specs/openapi.yaml and
specs/asyncapi.yaml; studio-v1.json against folo_source/legacy/v1.py; and the
committed schema bundle against the schema files. (Generated code is checked
by `scripts/contracts.py check`.)
"""

from __future__ import annotations

import json
import re
from pathlib import Path
from typing import Any

import pytest
import yaml

from folo_source.legacy import v1 as studio_config

ROOT = Path(__file__).resolve().parent.parent
DOCS = ROOT / 'docs' / 'specs'
SPECS = ROOT / 'specs'

from scripts import contracts  # noqa: E402

Json = Any
DEFS: dict[str, Json] = contracts.bundle()['$defs']


# ---------------------------------------------------------------- helpers


def resolve(schema: Json) -> Json:
    """A schema with $refs followed; siblings of a $ref (default) win."""
    while '$ref' in schema:
        target = DEFS[schema['$ref'].removeprefix('#/$defs/')]
        schema = {**target, **{k: v for k, v in schema.items() if k != '$ref'}}
    return schema


def field(definition: str, path: str) -> Json:
    """The schema of a dotted field path inside a definition."""
    schema = resolve(DEFS[definition])
    for part in path.split('.'):
        schema = resolve(schema['properties'][part])
    return schema


def table(text: str, marker: str) -> list[dict[str, str]]:
    """The first Markdown table after the line containing `marker`, as rows keyed by header."""
    lines = text[text.index(marker) :].splitlines()
    rows: list[list[str]] = []
    for line in lines[1:]:
        if line.startswith('|'):
            # Cells may hold escaped pipes (\|)
            cells = re.split(r'(?<!\\)\|', line.strip().strip('|'))
            rows.append([cell.strip().replace('\\|', '|') for cell in cells])
        elif rows:
            break
    header, _, *body = rows
    return [dict(zip(header, row, strict=True)) for row in body]


def plain(text: str) -> str:
    return text.replace('\u2212', '-').replace('`', '')  # minus sign


NUMBER = r'-?\d+(?:\.\d+)?'


def group(pattern: str, text: str) -> str:
    """The first group of `pattern` in `text`, which must match."""
    match = re.search(pattern, text)
    assert match, f'{pattern!r} not in {text!r}'
    return match.group(1)


def number_range(text: str) -> tuple[float, float]:
    # 0-1 or -24...24, written with an en dash or an ellipsis
    match = re.search(rf'({NUMBER})\s*[\u2013\u2026]\s*({NUMBER})', plain(text))
    assert match, f'no range in {text!r}'
    return float(match.group(1)), float(match.group(2))


def ticked(text: str) -> list[str]:
    return re.findall(r'`([^`]+)`', text)


def value(text: str) -> Json:
    """A default as the tables write it."""
    text = plain(text).strip()
    if text in ('true', 'false'):
        return text == 'true'
    if text in ('empty',):
        return ''
    if re.fullmatch(NUMBER, text):
        return float(text)
    if text.startswith('['):
        return json.loads(text)
    return text


def check_field(schema: Json, values: str, default: str) -> None:
    """One table row against its schema: range or choices, and default."""
    if 'enum' in schema:
        choices = ticked(values) or [w.strip() for w in plain(values).split('(')[0].split(',')]
        expected = [float(c) if re.fullmatch(NUMBER, c) else c for c in choices]
        assert schema['enum'] == expected, (values, schema['enum'])
    elif schema.get('type') in ('number', 'integer'):
        assert (schema['minimum'], schema['maximum']) == number_range(values), values
    elif schema.get('type') == 'boolean':
        assert values.startswith('boolean'), values
    elif schema.get('x-folo-color'):
        assert '#RRGGBB' in values, values
    assert schema['default'] == value(default), (default, schema['default'])


@pytest.fixture(scope='module')
def config_model() -> str:
    return (DOCS / 'config-model.md').read_text(encoding='utf-8')


# ---------------------------------------------------------------- configuration model


def test_zone_table(config_model: str) -> None:
    for row in table(config_model, '**Zone:**'):
        check_field(field('Zone', row['Field'].strip('`')), row['Range / values'], row['Default'])


def test_calibration_table(config_model: str) -> None:
    for row in table(config_model, '- `calibration`:'):
        name = row['Field'].split()[0].strip('`')
        schema = field('Calibration', name)
        if name == 'white_balance':
            for channel in 'rgb':
                part = resolve(schema['properties'][channel])
                assert (part['minimum'], part['maximum']) == number_range(row['Range / values'])
                assert part['default'] == 1
            continue
        check_field(schema, row['Range / values'], row['Default'])


def test_audio_visualizer_table(config_model: str) -> None:
    params = resolve(DEFS['AudioVisualizerParams'])
    for row in table(config_model, '**Audio visualizer parameters**'):
        names = [n.strip('` ') for n in row['Field'].split('(')[0].split(',')]
        values, default = row['Range / values'], row['Default']
        if names == ['freq_lo', 'freq_hi']:
            for name, expected in zip(names, default.split(','), strict=True):
                check_field(field('AudioVisualizerParams', name), values, expected)
            assert params['x-folo-band']['ratio'] == float(group(r'\u00d7 (\d+\.\d+)', values))
        elif names[0] in ('threshold', 'ceiling'):
            name = names[0]
            spec = params['x-folo-thresholds']
            level, hits = [plain(part) for part in values.split(';')[:2]]
            level_range, hits_range = number_range(level), number_range(hits)
            level_default, hits_default = (float(plain(d)) for d in default.split('/'))
            assert (
                spec['otherwise'][name]['minimum'],
                spec['otherwise'][name]['maximum'],
            ) == level_range
            assert (
                spec['by_trigger']['hits'][name]['minimum'],
                spec['by_trigger']['hits'][name]['maximum'],
            ) == hits_range
            assert spec['otherwise'][name]['default'] == level_default
            assert spec['by_trigger']['hits'][name]['default'] == hits_default
        elif names[0] == 'palette':
            schema = field('AudioVisualizerParams', 'palette')
            assert (schema['minItems'], schema['maxItems']) == tuple(
                int(n) for n in number_range(values)
            )
            assert schema['default'] == value(default)
        else:
            check_field(field('AudioVisualizerParams', names[0]), values, default)


def test_post_processing_table(config_model: str) -> None:
    by_effect: dict[str, str] = field('Layer', 'params')['x-folo-params-by-effect']
    rows = table(config_model, '**Post-processing parameters**')
    fields: dict[str, set[str]] = {}
    for row in rows:
        effect, name = row['Effect'].strip('`'), row['Field'].strip('`')
        fields.setdefault(effect, set()).add(name)
        definition = by_effect[effect].removeprefix('#/$defs/')
        check_field(field(definition, name), row['Range / values'], row['Default'])
    assert set(fields) == set(by_effect) - {'audio-visualizer'}
    for effect, names in fields.items():
        assert set(resolve(DEFS[by_effect[effect].removeprefix('#/$defs/')])['properties']) == names


def test_source_inputs_table(config_model: str) -> None:
    for row in table(config_model, '**Source** (in `sources.json`)'):
        name = row['Field'].strip('`')
        if name == 'screens[]':
            assert field('SourceInputs', 'screens')['default'] == []
            continue
        schema = field('SourceInputs', name)
        if schema.get('type') == 'string' and 'enum' not in schema:
            assert schema['default'] == (ticked(row['Default']) or [''])[0]
            continue
        check_field(schema, row['Range / values'], row['Default'])


def test_provider_table(config_model: str) -> None:
    kinds = {'Hyperion': 'HyperionSettings', 'WLED': 'WledSettings', 'Hue': 'HueSettings'}
    for row in table(config_model, '**Provider instance:**'):
        definition = kinds[row['Kind']]
        settings = resolve(DEFS[definition])['properties']
        names = [n for n in re.findall(r'`([a-z_]+)`', row['Settings']) if n in settings]
        defaults = [value(d) for d in row['Defaults'].split(',')]
        assert len(names) == len(defaults), row
        for name, default in zip(names, defaults, strict=True):
            assert resolve(settings[name])['default'] == default, (definition, name)
    hyperion = resolve(DEFS['HyperionSettings'])['properties']
    assert (hyperion['port']['minimum'], hyperion['port']['maximum']) == (1, 65535)
    assert (hyperion['priority']['minimum'], hyperion['priority']['maximum']) == (1, 253)
    assert hyperion['origin']['maxLength'] == 20


def test_node_settings_table(config_model: str) -> None:
    for row in table(config_model, '**Node settings**'):
        name = row['Field'].strip('`')
        schema = field('NodeSettings', name)
        if schema.get('type') == 'array':
            assert schema['maxItems'] == int(group(r'up to (\d+)', row['Range / values']))
            continue
        if schema.get('type') == 'string' and 'enum' not in schema:
            assert schema['default'] == (ticked(row['Default']) or [''])[0]
            continue
        check_field(schema, row['Range / values'], row['Default'])


def test_carried_over_limits(config_model: str) -> None:
    marker = '**Limits carried over:**'
    line = next(text for text in config_model.splitlines() if text.startswith(marker))
    numbers = [int(n) for n in re.findall(r'\b(\d+)\b', line)]
    assert numbers == [64, 16, 2000, 60, 16, 24], line
    layers = resolve(DEFS['Scene'])['properties']['layers_by_role']['additionalProperties']
    assert layers['maxItems'] == 64
    assert field('Layer', 'lane')['maximum'] + 1 == 16
    assert field('SurfaceResolution', 'width')['maximum'] == 2000
    assert field('StudioV1', 'presets')['maxItems'] == 60
    tags = resolve(DEFS['Tags'])
    assert (tags['maxItems'], resolve(tags['items'])['maxLength']) == (16, 24)


# ---------------------------------------------------------------- version 1


def test_studio_v1_schema_matches_sanitize_config() -> None:
    sc = studio_config
    segment = resolve(DEFS['V1Segment'])['properties']
    assert resolve(segment['mode'])['enum'] == list(sc.MODES)
    assert resolve(segment['direction'])['enum'] == list(sc.DIRECTIONS)
    assert resolve(segment['trigger'])['enum'] == list(sc.TRIGGERS)
    assert [float(b) for b in resolve(segment['beats'])['enum']] == list(sc.BEAT_DIVISIONS)
    assert resolve(DEFS['StudioV1'])['properties']['segments']['maxItems'] == sc.MAX_SEGMENTS
    assert resolve(segment['lane'])['maximum'] == sc.MAX_LANES - 1
    # The defaults: what sanitize_config makes of nothing
    defaults = sc.sanitize_config({})
    for key, schema in resolve(DEFS['StudioV1'])['properties'].items():
        if key in ('segments', 'presets'):
            continue
        assert resolve(schema)['default'] == defaults[key], key
    seg = sc.sanitize_segment({}, 2000)
    for key, schema in segment.items():
        if key == 'id':
            continue
        assert resolve(schema)['default'] == seg[key], key


def test_audio_visualizer_params_are_the_v1_segment_fields() -> None:
    """The v2 parameters are today's segment fields, unchanged."""
    v1 = resolve(DEFS['V1Segment'])['properties']
    for name, schema in resolve(DEFS['AudioVisualizerParams'])['properties'].items():
        mine, theirs = resolve(schema), resolve(v1[name])
        for key in ('type', 'enum', 'minimum', 'maximum', 'default', 'minItems', 'maxItems'):
            assert mine.get(key) == theirs.get(key), (name, key)


# ---------------------------------------------------------------- client API


def expand_paths(cell: str) -> list[str]:
    """`/zones/{id}/activate`, `/deactivate` -> both paths, the short form relative to the first."""
    paths = [p.replace('...', '{action}') for p in ticked(cell)]
    out = [paths[0]]
    for path in paths[1:]:
        if path.startswith('/{'):
            out.append(paths[0] + path)
        elif path.count('/') == 1:
            out.append(paths[0].rsplit('/', 1)[0] + path)
        else:
            out.append(path)
    return out


def test_client_api_resources_match_openapi() -> None:
    text = (DOCS / 'client-api.md').read_text(encoding='utf-8')
    spec = yaml.safe_load((SPECS / 'openapi.yaml').read_text(encoding='utf-8'))
    documented: set[str] = set()
    for row in table(text, '## Resources'):
        paths = expand_paths(row['Path'])
        methods = {m.strip().split(' ')[0].lower() for m in row['Methods'].split(',')}
        found: set[str] = set()
        for path in paths:
            assert path in spec['paths'], f'{path} is in client-api.md but not in openapi.yaml'
            found |= {
                m for m in spec['paths'][path] if m in ('get', 'post', 'put', 'delete', 'patch')
            }
            documented.add(path)
        assert found == methods, (row['Path'], sorted(found), sorted(methods))
    extra = {'/library/packs/{id}/export', '/live', '/ingest/surfaces', '/source-link'}
    assert set(spec['paths']) - documented == extra


def test_live_channel_matches_prose() -> None:
    text = (DOCS / 'client-api.md').read_text(encoding='utf-8')
    section = text[text.index('## Live WebSocket') : text.index('## Ingest')]
    topics = DEFS['Topic']['x-folo-topics']
    for topic in topics:
        assert f'`{topic}`' in section, topic
    types = [
        resolve(option)['properties']['type']['const']
        for option in DEFS['LiveServerMessage']['oneOf']
    ]
    for kind in types:
        assert f'`{kind}' in section, kind


# ---------------------------------------------------------------- source link


@pytest.fixture(scope='module')
def source_link() -> str:
    return (DOCS / 'source-link.md').read_text(encoding='utf-8')


def test_source_link_methods(source_link: str) -> None:
    methods: list[str] = []
    for row in table(source_link, '### Controller → source'):
        methods += [m.split(' ')[0] for m in ticked(row['Method'])]
    assert methods == DEFS['Method']['enum']
    notifications: list[str] = []
    for row in table(source_link, '### Source → controller'):
        names = ticked(row['Notification'])
        first = names[0].split(' ')[0]
        notifications.append(first)
        if '/' in row['Notification']:  # update.available / progress / result
            base = first.rsplit('.', 1)[0]
            notifications += [
                f'{base}.{part.strip()}' for part in row['Notification'].split('/')[1:]
            ]
    notifications = [n.strip('` ') for n in notifications]
    assert notifications == DEFS['NotificationMethod']['enum']
    asyncapi = yaml.safe_load((SPECS / 'asyncapi.yaml').read_text(encoding='utf-8'))
    for method in DEFS['Method']['enum']:
        assert method in asyncapi['components']['messages'], method
    for note in DEFS['NotificationMethod']['enum']:
        assert f'notify.{note}' in asyncapi['components']['messages'], note


def test_handshake_values(source_link: str) -> None:
    purposes = [resolve(o)['properties']['kind']['const'] for o in DEFS['Purpose']['oneOf']]
    for purpose in purposes:
        assert f'`{purpose}`' in source_link
    for result in DEFS['Welcome']['properties']['result']['enum']:
        assert f'`{result}`' in source_link


@pytest.mark.parametrize(
    ('marker', 'definition'),
    [
        ('## Surface frames', 'SurfaceFrameHeader'),
        ('## Signal frames', 'SignalFrameHeader'),
    ],
)
def test_frame_layouts(source_link: str, marker: str, definition: str) -> None:
    rows = table(source_link, marker)
    layout = DEFS[definition]['x-folo-layout']
    header = [
        r for r in rows if r['Size'] != 'n' and '\u00d7' not in r['Size']
    ]  # not the 4 x count values
    assert [(int(r['Offset']), int(r['Size'])) for r in header] == [
        (f['offset'], f['size']) for f in layout
    ]
    assert int(rows[-1]['Offset']) == DEFS[definition]['x-folo-header-size']
    for row, part in zip(header, layout, strict=True):
        if 'value' in part and isinstance(part['value'], str):
            assert part['value'] in row['Field']


# ---------------------------------------------------------------- the bundle


def test_bundle_is_up_to_date() -> None:
    committed = json.loads(contracts.BUNDLE.read_text(encoding='utf-8'))
    assert committed == contracts.bundle(), 'run `uv run python scripts/contracts.py generate`'
