"""
The legacy studio's Hyperion output: sends the latest frame as JSON color
commands. The controller takes this over (RB-10).
"""

import threading
import time

from folo_source.legacy.json_client import JsonClient

# Hyperion drops our colors after this many ms without updates, so the leds
# are released even if the studio is killed
HYPERION_DURATION = 2000
# An unchanged frame (silence, a solid ambient light) is sent again only this
# often, well within HYPERION_DURATION and WLED's live mode timeout (2.5 s)
KEEPALIVE = 0.5
RECONNECT_DELAY = 5.0


class HyperionSender:
    """
    Sends the latest frame to hyperion at the configured rate, when it has
    changed or KEEPALIVE seconds after it was last sent.
    """

    def __init__(self, engine):
        self.engine = engine
        self.status = 'starting'
        self.running = False
        self.client = None
        self.target = None
        self.thread = None

    def start(self):
        self.running = True
        self.thread = threading.Thread(target=self._loop, name='hyperion', daemon=True)
        self.thread.start()

    def stop(self):
        """Asks the thread to stop; it clears the colors on its way out. Join `thread`."""
        self.running = False

    def _close(self):
        if self.client is not None:
            self.client.disconnect()
            self.client = None

    def _loop(self):
        retry_at = 0.0
        sent, sent_at = None, 0.0
        while self.running:
            settings = self.engine.store.config['hyperion']
            if not settings['enabled']:
                self._close()
                self.status = 'off'
                time.sleep(0.2)
                continue

            target = (settings['host'], settings['port'], settings['priority'])
            if self.client is not None and target != self.target:
                self._close()

            if self.client is None:
                if time.time() < retry_at:
                    time.sleep(0.2)
                    continue
                client = JsonClient(
                    settings['host'],
                    settings['port'],
                    timeout=3,
                    priority=settings['priority'],
                    duration=HYPERION_DURATION,
                )
                client.connect()
                if not client.connected:
                    self.status = 'disconnected'
                    retry_at = time.time() + RECONNECT_DELAY
                    continue
                self.client = client
                self.target = target
                self.status = 'connected'
                sent = None

            frame = self.engine.frame
            now = time.perf_counter()
            if frame and (frame != sent or now - sent_at >= KEEPALIVE):
                self.client.send_led_data(bytearray(frame))
                if not self.client.connected:
                    self.client = None
                    self.status = 'disconnected'
                    retry_at = time.time() + RECONNECT_DELAY
                    continue
                sent, sent_at = frame, now
            time.sleep(1.0 / settings['fps'])
        self._close()
