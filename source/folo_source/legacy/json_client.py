"""
This module is used to send the led data to hyperion's json server

Created on 27.11.2014, last updated on 02.02.2015

@author: Fabian Hertwig
@author: Juha Rantanen
"""

import json
import logging
import socket
import threading

log = logging.getLogger(__name__)

ORIGIN = 'Audio Effects'
READER_JOIN = 1.0  # seconds the reply reader gets to end once the socket is closed


def fetch_server_info(host, port, timeout=10):
    """
    Returns the serverinfo of a running hyperion (Hyperion.NG JSON API).
    """
    sock = socket.create_connection((host, port), timeout)
    try:
        sock.sendall(b'{"command":"serverinfo","tan":1}\n')
        buf = b''
        while not buf.endswith(b'\n'):
            chunk = sock.recv(65536)
            if not chunk:
                break
            buf += chunk
    finally:
        sock.close()
    reply = json.loads(buf.decode('utf-8'))
    if not reply.get('success'):
        raise RuntimeError('serverinfo failed: {}'.format(reply.get('error')))
    return reply['info']


class JsonClient:
    def __init__(self, host, port, timeout=10, priority=100, duration=None):

        self.host = host
        self.port = port
        self.timeout = timeout
        self.priority = priority
        # Milliseconds hyperion keeps the colors if no new data arrives
        self.duration = duration
        self.connected = False
        self.socket: socket.socket | None = None
        self._reader: threading.Thread | None = None

    def __del__(self):
        self.disconnect()

    def connect(self):
        """
        Open a socket connection to the server
        :param host: Hostname or IP address of the server
        :param port: Port of the server
        :param timeout: timeout in seconds
        """

        if self.connected:
            return

        self.socket = socket.socket()
        self.socket.settimeout(self.timeout)
        try:
            self.socket.connect((self.host, self.port))
            self.connected = True
            # Hyperion answers every command, read and discard the replies so
            # the socket buffers never fill up
            self._reader = threading.Thread(
                target=self._drain, args=(self.socket,), name='hyperion-replies', daemon=True
            )
            self._reader.start()
        except OSError as exc:
            log.warning('could not connect to Hyperion at %s:%s: %s', self.host, self.port, exc)

    def _drain(self, sock):
        try:
            while sock.recv(65536):
                pass
        except (OSError, ValueError):
            pass

    def send(self, command):
        assert self.socket is not None, 'not connected'
        self.socket.sendall((json.dumps(command) + '\n').encode('ascii'))

    def disconnect(self):
        if self.connected:
            try:
                self.send({'command': 'clear', 'priority': self.priority})
                if self.socket is not None:
                    self.socket.close()
                self.connected = False
            except OSError as exc:
                log.warning('could not close the connection to Hyperion: %s', exc)
        reader = self._reader
        if reader is not None and reader is not threading.current_thread():
            reader.join(READER_JOIN)

    def send_led_data(self, led_data):
        """
        Send the led data in a message format the hyperion json server understands
        :param led_data: bytearray of the led data (r,g,b) * hyperion.ledcount
        """

        if not self.connected:
            self.connect()
            if not self.connected:
                return
        if not led_data:
            return
        message = {
            'command': 'color',
            'color': list(led_data),
            'priority': self.priority,
            'origin': ORIGIN,
        }
        if self.duration:
            message['duration'] = self.duration
        try:
            self.send(message)
        except OSError as exc:
            log.warning('could not send the LED data to Hyperion: %s', exc)
            # Recreate the socket
            self.connected = False
            log.info('reconnecting to Hyperion')
            self.connect()
            if self.connected:
                log.info('reconnected to Hyperion')
