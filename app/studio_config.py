"""
Configuration of the FOLO studio: segments that map a frequency range of the
audio to a range of leds. Everything coming from the web UI goes through
sanitize_config() so the engine can trust the values.
"""

import json
import math
import os
import re
import threading
import uuid

HEX_COLOR = re.compile(r'^#[0-9a-fA-F]{6}$')

# The last six move by themselves, after LedFx effects (see studio_engine)
MODES = ('fill', 'pulse', 'spectrum', 'shift', 'scroll', 'drops', 'scan', 'wipe', 'flow', 'fire')
DIRECTIONS = ('forward', 'reverse', 'center', 'edges')
# level: threshold and ceiling are dB levels of the band.
# hits: they are dB above the band's recent average, so the segment reacts to
# sudden peaks whatever the song's volume.
TRIGGERS = ('level', 'hits')
HITS_MAX_DB = 40.0

MIN_HZ = 10.0
MAX_HZ = 24000.0
MIN_DB = -120.0
MAX_SPEED = 2000.0          # leds per second of the moving modes
MAX_SEGMENTS = 64
MAX_LANES = 16              # rows of the web UI's LED track
MAX_LEDS = 2000
MAX_PRESETS = 60
MAX_TAGS = 16
MAX_TAG_LENGTH = 24

SAVE_DELAY = 0.5


def _float(value, default, lo, hi):
    try:
        number = float(value)
    except (TypeError, ValueError):
        return default
    if not math.isfinite(number):
        return default
    return max(lo, min(hi, number))


def _int(value, default, lo, hi):
    return int(round(_float(value, default, lo, hi)))


def _color(value, default):
    if isinstance(value, str) and HEX_COLOR.match(value):
        return value.upper()
    return default


def new_id():
    return uuid.uuid4().hex[:8]


def sanitize_segment(seg, led_count):
    last = max(0, led_count - 1)
    start = _int(seg.get('start'), 0, 0, last)
    end = _int(seg.get('end'), last, 0, last)
    if end < start:
        start, end = end, start

    freq_lo = _float(seg.get('freq_lo'), 60.0, MIN_HZ, MAX_HZ)
    freq_hi = _float(seg.get('freq_hi'), 250.0, MIN_HZ, MAX_HZ)
    if freq_hi < freq_lo:
        freq_lo, freq_hi = freq_hi, freq_lo
    if freq_hi < freq_lo * 1.01:
        freq_hi = min(MAX_HZ, freq_lo * 1.01)
        freq_lo = freq_hi / 1.01

    trigger = seg.get('trigger')
    trigger = trigger if trigger in TRIGGERS else 'level'
    if trigger == 'hits':
        threshold = _float(seg.get('threshold'), 3.0, 0.0, HITS_MAX_DB - 1.0)
        ceiling = _float(seg.get('ceiling'), 10.0, 1.0, HITS_MAX_DB)
    else:
        threshold = _float(seg.get('threshold'), -60.0, MIN_DB, -1.0)
        ceiling = _float(seg.get('ceiling'), -25.0, MIN_DB + 1.0, 0.0)
    if ceiling < threshold + 1.0:
        ceiling = threshold + 1.0

    palette = [_color(c, None) for c in (seg.get('palette') or [])]
    palette = [c for c in palette if c][:16]

    mode = seg.get('mode')
    direction = seg.get('direction')

    return {
        'id': str(seg.get('id') or new_id())[:32],
        'name': str(seg.get('name') or 'Segment')[:40],
        'enabled': bool(seg.get('enabled', True)),
        'solo': bool(seg.get('solo', False)),
        'start': start,
        'end': end,
        'freq_lo': round(freq_lo, 2),
        'freq_hi': round(freq_hi, 2),
        'trigger': trigger,
        'threshold': round(threshold, 1),
        'ceiling': round(ceiling, 1),
        'gain': _float(seg.get('gain'), 1.0, 0.1, 8.0),
        'curve': _float(seg.get('curve'), 1.0, 0.2, 5.0),
        'attack': _float(seg.get('attack'), 15.0, 0.0, 2000.0),
        'release': _float(seg.get('release'), 200.0, 0.0, 5000.0),
        'floor': _float(seg.get('floor'), 0.0, 0.0, 1.0),
        'mode': mode if mode in MODES else 'fill',
        'direction': direction if direction in DIRECTIONS else 'forward',
        'peak': bool(seg.get('peak', False)),
        # moving modes: speed in leds/second, width in percent of the segment
        'speed': _float(seg.get('speed'), 120.0, 0.0, MAX_SPEED),
        'width': _float(seg.get('width'), 20.0, 1.0, 100.0),
        'palette': palette or ['#FFFFFF'],
        # row on the web UI's LED track, -1 to place it automatically
        'lane': _int(seg.get('lane'), -1, -1, MAX_LANES - 1),
    }


def sanitize_config(cfg, led_count=None):
    """
    Returns a complete, valid config. led_count overrides the stored count.
    """
    if not isinstance(cfg, dict):
        cfg = {}
    if led_count is None:
        led_count = cfg.get('led_count')
    led_count = _int(led_count, 150, 1, MAX_LEDS)

    audio = cfg.get('audio') if isinstance(cfg.get('audio'), dict) else {}
    hyperion = cfg.get('hyperion') if isinstance(cfg.get('hyperion'), dict) else {}
    segments = cfg.get('segments') if isinstance(cfg.get('segments'), list) else []

    clean = _sanitize_segments(segments, led_count)

    presets = cfg.get('presets') if isinstance(cfg.get('presets'), list) else []
    presets = [sanitize_preset(p) for p in presets[:MAX_PRESETS] if isinstance(p, dict)]
    preset_ids = set()
    for preset in presets:
        if preset['id'] in preset_ids:
            preset['id'] = new_id()
        preset_ids.add(preset['id'])

    return {
        'version': 1,
        'led_count': led_count,
        'audio': {
            'device': str(audio.get('device') or 'default')[:200],
        },
        'hyperion': {
            'enabled': bool(hyperion.get('enabled', True)),
            'host': str(hyperion.get('host') or 'localhost')[:255],
            'port': _int(hyperion.get('port'), 19444, 1, 65535),
            'priority': _int(hyperion.get('priority'), 100, 1, 253),
            'fps': _int(hyperion.get('fps'), 40, 5, 60),
        },
        'master': _float(cfg.get('master'), 1.0, 0.0, 1.0),
        'background': _color(cfg.get('background'), '#000000'),
        'segments': clean,
        'presets': presets,
        # id of a saved preset or a built-in one (built-ins live in the web UI)
        'default_preset': str(cfg.get('default_preset') or '')[:64],
    }


def _sanitize_segments(segments, led_count):
    seen = set()
    clean = []
    for seg in segments[:MAX_SEGMENTS]:
        if not isinstance(seg, dict):
            continue
        seg = sanitize_segment(seg, led_count)
        if seg['id'] in seen:
            seg['id'] = new_id()
        seen.add(seg['id'])
        clean.append(seg)
    return clean


def sanitize_tags(tags):
    """
    Up to MAX_TAGS distinct, trimmed tags (case-insensitive duplicates dropped).
    """
    clean = []
    seen = set()
    for tag in tags if isinstance(tags, list) else []:
        if not isinstance(tag, str):
            continue
        tag = ' '.join(tag.split())[:MAX_TAG_LENGTH]
        if tag and tag.lower() not in seen:
            seen.add(tag.lower())
            clean.append(tag)
    return clean[:MAX_TAGS]


def sanitize_preset(preset):
    """
    A saved preset: segments for the led count it was made with, plus tags.
    """
    led_count = _int(preset.get('led_count'), 150, 1, MAX_LEDS)
    segments = preset.get('segments') if isinstance(preset.get('segments'), list) else []
    return {
        'id': str(preset.get('id') or new_id())[:64],
        'name': (str(preset.get('name') or '').strip() or 'My preset')[:40],
        'desc': str(preset.get('desc') or '')[:200],
        'tags': sanitize_tags(preset.get('tags')),
        'led_count': led_count,
        'segments': _sanitize_segments(segments, led_count),
    }


def scale_segments(cfg, led_count):
    """
    Stretches the segments of a sanitized config to a new led count, so they
    keep their share of the strip.
    """
    old_count = cfg['led_count']
    if old_count == led_count:
        return cfg
    ratio = led_count / float(old_count)
    segments = []
    for seg in cfg['segments']:
        start = int(round(seg['start'] * ratio))
        end = max(start, int(round((seg['end'] + 1) * ratio)) - 1)
        segments.append(dict(seg, start=start, end=end))
    return dict(cfg, segments=segments)


def default_config(led_count):
    """
    Bass, mids and highs over thirds of the strip.
    """
    third = max(1, led_count // 3)
    segments = [
        {'name': 'Bass', 'start': 0, 'end': third - 1,
         'freq_lo': 25, 'freq_hi': 160, 'threshold': -50, 'ceiling': -15,
         'mode': 'fill', 'direction': 'center', 'peak': True,
         'attack': 10, 'release': 250,
         'palette': ['#0033CC', '#000080', '#4B00FF']},
        {'name': 'Mids', 'start': third, 'end': 2 * third - 1,
         'freq_lo': 160, 'freq_hi': 2000, 'threshold': -55, 'ceiling': -25,
         'mode': 'pulse', 'attack': 20, 'release': 300,
         'palette': ['#FF3399', '#FF00FF', '#800080']},
        {'name': 'Highs', 'start': 2 * third, 'end': led_count - 1,
         'freq_lo': 2000, 'freq_hi': 16000, 'threshold': -72, 'ceiling': -40,
         'mode': 'spectrum', 'attack': 5, 'release': 180,
         'palette': ['#800080', '#8F00FF', '#4B0082']},
    ]
    return sanitize_config({'segments': segments}, led_count)


class ConfigStore(object):
    """
    Holds the current config and saves it to disk shortly after changes.
    The config dict is replaced as a whole on every change, so readers can
    use a reference to it without locking.
    """
    def __init__(self, path):
        self.path = path
        self.config = None
        # Pages compare these to notice changes made elsewhere
        self.server_id = new_id()
        self.revision = 0
        self._lock = threading.Lock()
        self._timer = None

    def versioned(self):
        """
        The config as sent to the web UI, tagged with its revision.
        """
        return dict(self.config, revision=self.revision, server_id=self.server_id)

    def load(self, lookup_led_count):
        """
        Loads the config file (or creates the default). lookup_led_count is
        called with the hyperion settings and returns the led count of the
        running hyperion, or None if it can't be reached.
        """
        data = None
        if os.path.exists(self.path):
            try:
                with open(self.path) as config_file:
                    data = json.load(config_file)
            except (OSError, ValueError) as exc:
                print('Could not read {}: {}'.format(self.path, exc))
        stored = sanitize_config(data)
        led_count = lookup_led_count(stored['hyperion']) or stored['led_count']
        if data is None:
            self.config = default_config(led_count)
        else:
            self.config = sanitize_config(scale_segments(stored, led_count), led_count)
        self.save()
        return self.config

    def update(self, data):
        """
        Replaces the config. A client that hasn't seen a led count change yet
        sends its own count, and its segments are scaled to the current one.
        """
        led_count = self.config['led_count']
        posted_count = data.get('led_count') if isinstance(data, dict) else None
        cfg = sanitize_config(data, posted_count or led_count)
        self.config = sanitize_config(scale_segments(cfg, led_count), led_count)
        self._changed()
        return self.config

    def reset(self):
        """
        Back to the default segments, keeping the audio and hyperion settings
        and the saved presets.
        """
        fresh = default_config(self.config['led_count'])
        for key in ('audio', 'hyperion', 'presets', 'default_preset'):
            fresh[key] = self.config[key]
        return self.update(fresh)

    def set_led_count(self, led_count):
        self.config = sanitize_config(scale_segments(self.config, led_count), led_count)
        self._changed()
        return self.config

    def _changed(self):
        self.revision += 1
        with self._lock:
            if self._timer is not None:
                self._timer.cancel()
            self._timer = threading.Timer(SAVE_DELAY, self.save)
            self._timer.daemon = True
            self._timer.start()

    def save(self):
        with self._lock:
            if self._timer is not None:
                self._timer.cancel()
                self._timer = None
            tmp_path = self.path + '.tmp'
            with open(tmp_path, 'w') as config_file:
                json.dump(self.config, config_file, indent=2)
            os.replace(tmp_path, self.path)
