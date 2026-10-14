"""
FOLO studio (Fear of lights out)

A local web UI for building audio reactive led segments: pick led ranges,
assign frequency ranges with thresholds, and see the result live in the
browser and on the leds through hyperion.

Usage: python studio.py [--port 8777] [--no-browser]
Stop with Ctrl+C.
"""

import argparse
import json
import os
import sys
import threading
import time
import webbrowser
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

BASE_DIR = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, BASE_DIR)

from app.json_client import fetch_server_info
from app.studio_config import ConfigStore
from app.studio_engine import (
    DISPLAY_BANDS,
    DISPLAY_MAX_HZ,
    DISPLAY_MIN_HZ,
    StudioEngine,
    audio_sources,
)

WEB_DIR = os.path.join(BASE_DIR, 'web')
STATIC_FILES = {
    '/': ('index.html', 'text/html; charset=utf-8'),
    '/index.html': ('index.html', 'text/html; charset=utf-8'),
    '/studio.js': ('studio.js', 'application/javascript; charset=utf-8'),
    '/studio.css': ('studio.css', 'text/css; charset=utf-8'),
}
# Requests with any other Host header are refused (DNS rebinding protection)
LOCAL_HOSTS = ('127.0.0.1', 'localhost', '[::1]')
STREAM_FPS = 30
MAX_BODY = 1000000
LAYOUT_POLL = 5.0


def fetch_led_count(hyperion, quiet=False):
    """
    Led count of the running hyperion, or None if it can't be reached.
    """
    try:
        return len(fetch_server_info(hyperion['host'], hyperion['port'], timeout=5)['leds'])
    except (OSError, ValueError, KeyError, RuntimeError) as exc:
        if not quiet:
            print(f'Could not read the led layout from hyperion: {exc}')
        return None


def watch_layout(store, stopping):
    """
    Follows led count changes in hyperion, e.g. after editing its led layout.
    """
    while not stopping.wait(LAYOUT_POLL):
        led_count = fetch_led_count(store.config['hyperion'], quiet=True)
        if led_count and led_count != store.config['led_count']:
            print(
                'Hyperion now has {} leds (was {}), scaling the segments to fit'.format(
                    led_count, store.config['led_count']
                )
            )
            store.set_led_count(led_count)


class StudioServer(ThreadingHTTPServer):
    """The HTTP server, holding what the handlers use."""

    daemon_threads = True

    def __init__(self, address: tuple[str, int], store: ConfigStore, engine: StudioEngine) -> None:
        super().__init__(address, StudioHandler)
        self.store = store
        self.engine = engine
        self.stopping = False


class StudioHandler(BaseHTTPRequestHandler):
    server_version = 'HyperionAudioStudio/1.0'
    server: StudioServer

    def log_message(self, format, *args):  # pylint: disable=redefined-builtin
        pass

    def _local(self):
        host = self.headers.get('Host', '')
        name = host.split(']')[0] + ']' if host.startswith('[') else host.split(':')[0]
        if name in LOCAL_HOSTS:
            return True
        self._send_json({'error': 'forbidden'}, 403)
        return False

    def _send(self, status, body, content_type):
        self.send_response(status)
        self.send_header('Content-Type', content_type)
        self.send_header('Content-Length', str(len(body)))
        self.send_header('Cache-Control', 'no-store')
        self.end_headers()
        self.wfile.write(body)

    def _send_json(self, data, status=200):
        self._send(status, json.dumps(data).encode('utf-8'), 'application/json')

    @staticmethod
    def _sources():
        try:
            return audio_sources()
        except Exception as exc:  # pylint: disable=broad-except
            print(f'Could not list audio sources: {exc}')
            return {'default': '', 'devices': [], 'apps': []}

    def _state(self):
        sources = self._sources()
        return {
            'config': self.server.store.versioned(),
            'devices': sources['devices'],
            'default_device': sources['default'],
            'apps': sources['apps'],
            'display': {
                'bands': DISPLAY_BANDS,
                'min_hz': DISPLAY_MIN_HZ,
                'max_hz': DISPLAY_MAX_HZ,
            },
        }

    def do_GET(self):  # pylint: disable=invalid-name
        if not self._local():
            return
        path = self.path.split('?', 1)[0]
        if path in STATIC_FILES:
            name, content_type = STATIC_FILES[path]
            with open(os.path.join(WEB_DIR, name), 'rb') as static_file:
                self._send(200, static_file.read(), content_type)
        elif path == '/api/state':
            self._send_json(self._state())
        elif path == '/api/sources':
            # devices and the apps playing audio, refreshed by the Audio input card
            self._send_json(self._sources())
        elif path == '/api/stream':
            self._stream()
        elif path == '/api/art':
            # the album art of what is playing (the page adds ?<art id> to skip its cache)
            art = self.server.engine.media.art
            if art:
                self._send(200, art[0], art[1])
            else:
                self._send_json({'error': 'no album art'}, 404)
        else:
            self._send_json({'error': 'not found'}, 404)

    def do_POST(self):  # pylint: disable=invalid-name
        if not self._local():
            return
        if not self.headers.get('Content-Type', '').startswith('application/json'):
            self._send_json({'error': 'expected application/json'}, 415)
            return
        length = int(self.headers.get('Content-Length') or 0)
        if length > MAX_BODY:
            self._send_json({'error': 'request too large'}, 413)
            return
        try:
            data = json.loads(self.rfile.read(length).decode('utf-8') or '{}')
        except ValueError:
            self._send_json({'error': 'invalid json'}, 400)
            return

        path = self.path.split('?', 1)[0]
        store = self.server.store
        if path == '/api/config':
            store.update(data)
            self._send_json(store.versioned())
        elif path == '/api/reset':
            store.reset()
            self._send_json(store.versioned())
        elif path == '/api/media':
            # play_pause, next or previous on the player shown as now playing
            done = self.server.engine.media.control(
                data.get('action') if isinstance(data, dict) else None
            )
            self._send_json({'ok': done})
        elif path == '/api/layout':
            led_count = fetch_led_count(store.config['hyperion'])
            if led_count:
                store.set_led_count(led_count)
            state = self._state()
            state['layout_ok'] = led_count is not None
            self._send_json(state)
        else:
            self._send_json({'error': 'not found'}, 404)

    def _stream(self):
        """
        Server-sent events with the live spectrum, leds and levels.
        """
        self.send_response(200)
        self.send_header('Content-Type', 'text/event-stream')
        self.send_header('Cache-Control', 'no-store')
        self.end_headers()
        try:
            while not self.server.stopping:
                payload = json.dumps(self.server.engine.snapshot(), separators=(',', ':'))
                self.wfile.write(f'data: {payload}\n\n'.encode())
                self.wfile.flush()
                time.sleep(1.0 / STREAM_FPS)
        except OSError:
            pass


def main():
    parser = argparse.ArgumentParser(
        description='FOLO, fear of lights out: audio-reactive LEDs for Hyperion'
    )
    parser.add_argument('--port', type=int, default=8777, help='port of the web UI (default 8777)')
    parser.add_argument(
        '--config',
        default=os.path.join(BASE_DIR, 'studio.json'),
        help='segments file (default studio.json)',
    )
    parser.add_argument(
        '--no-browser', action='store_true', help="don't open the UI in the browser"
    )
    args = parser.parse_args()

    store = ConfigStore(args.config)
    store.load(fetch_led_count)
    print('Leds: {}, segments: {}'.format(store.config['led_count'], len(store.config['segments'])))

    engine = StudioEngine(store)
    engine.start()

    stopping = threading.Event()
    watcher = threading.Thread(target=watch_layout, args=(store, stopping))
    watcher.daemon = True
    watcher.start()

    server = StudioServer(('127.0.0.1', args.port), store, engine)

    url = f'http://127.0.0.1:{args.port}/'
    print(f'Audio studio running at {url}')
    print('Stop with Ctrl+C')
    if not args.no_browser:
        webbrowser.open(url)

    try:
        server.serve_forever()
    except KeyboardInterrupt:
        pass
    finally:
        server.stopping = True
        stopping.set()
        engine.stop()
        store.save()
        server.server_close()
        print('Stopped')


if __name__ == '__main__':
    main()
