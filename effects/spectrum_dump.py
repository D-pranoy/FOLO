### Spectrum Dump ####
# Originally a GStreamer 1.0 spectrum/level reader by RanzQ, based on
# https://github.com/Wintervenom/gst-spectrumdump by Scott Garrett.
#
# Ported to Python 3 / Windows: audio is captured from a WASAPI loopback
# device (what you hear) with the `soundcard` library, and the GStreamer
# `spectrum` and `level` elements are emulated with numpy so the effects
# receive magnitudes in the same format as before.
################################
# Dependencies:
# numpy, soundcard
#################################

import sys
import math
import ctypes
import warnings
from threading import Thread, current_thread

import numpy as np
import soundcard as sc

warnings.filterwarnings('ignore', category=sc.SoundcardRuntimeWarning)

SAMPLERATE = 44100

# GStreamer level element defaults
PEAK_TTL = 0.3          # seconds the decay value holds the peak
PEAK_FALLOFF = 10.0     # dB per second after the hold

SILENCE_DB = -200.0


def stdout(message):
    """
    Writes a message to STDOUT.
    """
    sys.stdout.write("{0}\n".format(message))
    sys.stdout.flush()


def stderr(message):
    """
    Writes a message to STDERR.
    """
    sys.stderr.write("{0}\n".format(message))
    sys.stderr.flush()


def list_devices():
    """
    Prints the capture devices that can be used as audio source.
    """
    default = sc.default_speaker()
    stdout('Default output (used for "autoaudiosrc"/"default"): {}'.format(default.name))
    stdout('Available sources (loopback devices capture what is played on them):')
    for mic in sc.all_microphones(include_loopback=True):
        kind = 'loopback' if mic.isloopback else 'input'
        stdout('  [{}] {}'.format(kind, mic.name))


def find_device(source):
    """
    Resolves an audio source string to a soundcard microphone object.
    "autoaudiosrc", "default" or empty selects the loopback of the default
    speaker. Anything else is matched (case insensitive substring) against
    device names, loopback devices first.
    """
    if not source or source in ('autoaudiosrc', 'default', 'loopback'):
        return sc.get_microphone(id=str(sc.default_speaker().name), include_loopback=True)

    wanted = source.lower()
    mics = sc.all_microphones(include_loopback=True)
    mics.sort(key=lambda m: not m.isloopback)
    for mic in mics:
        if wanted in mic.name.lower():
            return mic
    raise ValueError('Audio source "{}" not found, run main.py --list-devices'.format(source))


def _init_com():
    # WASAPI needs COM initialised in every thread that uses it
    try:
        ctypes.windll.ole32.CoInitializeEx(None, 0)
    except AttributeError:
        pass


class GstSpectrumDump(object):
    """
    Dumps the spectrum magnitudes of incoming audio as volume units per band.

    Optional arguments:
    <source>        Audio source: "autoaudiosrc"/"default" for the loopback of
                    the default speaker, or part of a device name.
    <precision>     How many decimal places to round the magnitudes to
                    (default: 16).
    <bands>         How many frequency bands to output (default: 128).
    <amplify>       Amplify output by this much (default: 1).
    <logamplify>    Amplify magnitude values logarithmically to compensate for
                    softer higher frequencies.  (default: False)
    <autoamp>       Automatically control amplification levels when they are
                    too loud.
    <threshold>     Minimal magnitude of a band in decibels (default: 70).
    <cufoff>        Cut off magnitudes at this value after amplification has
                    been applied (default: 100).
    <scale>         Scale magnitudes to this value (default: 100).
    <raw>           Don't clip or apply logarithmic upscale the output.
                    (default: True).
    <db>            Return output in decibels instead of a percentage.
                    <logamplify> is ignored (default: False).
    <iec>           Convert decibels to percentages with IEC 60268-18 scaling
                    (default: False).
    <vumeter>       Return VU meter output instead of spectrum.  <bands>
                    controls how many channels to output here.  <threshold> is
                    ignored.
    <interval>      Milliseconds to wait between polls (default: 50).
    <multichannel>  Spectrum from multiple channels? (default: False)
    <quiet>         Don't output to STDERR (default: False if no callback).
    <callback>      Return the magnitude list to this function (default: None).
    """
    def __init__(self, **opts):
        self.running = False
        self.source = opts.get('source')
        self.precision = opts.get('precision')
        self.bands = opts.get('bands', 128)
        self.amplify = opts.get('amplify', 1)
        self.logamplify = opts.get('logamplify', False)
        self.autoamp = opts.get('autoamp', False)
        self.threshold = opts.get('threshold', 70)
        self.cutoff = opts.get('cutoff', 100)
        self.scaleto = opts.get('scale', 100)
        self.raw = opts.get('raw', True)
        self.db = opts.get('db', False)
        self.iec = opts.get('iec', False)
        self.vumeter = opts.get('vumeter', False)
        self.interval = opts.get('interval', 50)
        self.callback = opts.get('callback')
        self.multichannel = opts.get('multichannel', False)
        self.bands_cutoff = opts.get('cutoff', 96)
        self.quiet = opts.get('quiet', self.callback is not None)
        self.gainhits = 0
        self.origamp = self.amplify
        self.device = find_device(self.source)
        self.thread = None

        # Spectrum state
        self.nfft = 2 * self.bands - 2
        self.window = np.hamming(self.nfft) if self.nfft > 0 else None
        self.pending = np.zeros(0, dtype=np.float32)

        # Level state (per channel)
        self.decay_db = None
        self.peak_age = None


    def round(self, n):
        if self.precision:
            return round(n, self.precision)
        elif self.precision == 0:
            return int(n)
        return n


    def dbtopct(self, db, index=None):
        indexamp = 1
        if self.iec:
            pct = 0.0
            if db < -70.0:
                pct = 0.0
            elif db < -60.0:
                pct = (db + 70.0) * 0.25
            elif db < -50.0:
                pct = (db + 60.0) * 0.5 + 2.5
            elif db < -40.0:
                pct = (db + 50.0) * 0.75 + 7.5
            elif db < -30.0:
                pct = (db + 40.0) * 1.5 + 15.0
            elif db < -20.0:
                pct = (db + 30.0) * 2.0 + 30.0
            elif db < 0.0:
                pct = (db + 20.0) * 2.5 + 50.0
            else:
                pct = 100.0
        else:
            pct = (self.threshold + db) / float(self.threshold) * 100
        if index and index > 0:
            indexamp += math.log10(index)
        pct = min(self.cutoff, self.amplify * (indexamp * pct))
        if self.autoamp:
            if pct == 100:
                self.gainhits += 1
                if self.amplify > 0:
                    self.amplify -= 0.1
            elif pct == 0:
                self.gainhits -= 1
                if self.gainhits < -100:
                    if self.amplify < self.origamp:
                        self.amplify += 0.01
                    self.gainhits = 0
        return pct * (self.scaleto / 100.0)


    def interpolate(self, a, b, points):
        points = round(points, 0) + 1.0
        return [a + ((b - a) / points) * x for x in range(0, int(points))]


    def scale(self, floats, maxlen=None):
        if len(floats) < 2:
            return floats
        scaled = []
        for i in range(1, len(floats)):
            length = 1 + math.log10(i - 0)
            scaled += self.interpolate(floats[i-1], floats[i], length)[:-1]
        scaled.append(floats[-1])
        if maxlen and len(scaled) > maxlen:
            downscaled = []
            incr = len(scaled) / float(maxlen)
            index = 0
            for _ in range(0, maxlen):
                downscaled.append(scaled[int(round(index, 0))])
                index += incr
            return downscaled
        else:
            return scaled


    def compute_spectrum(self, frames):
        """
        Emulates the GStreamer spectrum element: averaged power of Hamming
        windowed FFTs of size 2*bands-2, in dB, clamped to -threshold.
        Returns None if not enough samples were collected yet.
        """
        mono = frames.mean(axis=1) if frames.ndim > 1 else frames
        data = np.concatenate((self.pending, mono.astype(np.float32)))
        chunks = len(data) // self.nfft
        if chunks == 0:
            self.pending = data
            return None
        self.pending = data[chunks * self.nfft:]
        blocks = data[:chunks * self.nfft].reshape(chunks, self.nfft) * self.window
        spec = np.fft.rfft(blocks, axis=1)
        power = (spec.real ** 2 + spec.imag ** 2).mean(axis=0) / float(self.nfft * self.nfft)
        with np.errstate(divide='ignore'):
            mags = 10.0 * np.log10(power)
        mags = np.maximum(mags, -float(self.threshold))
        return mags[:self.bands].tolist()


    def compute_level(self, frames):
        """
        Emulates the GStreamer level element: peak and decaying peak in dB
        per channel.
        """
        dt = len(frames) / float(SAMPLERATE)
        channels = frames.shape[1]
        if self.decay_db is None:
            self.decay_db = [SILENCE_DB] * channels
            self.peak_age = [0.0] * channels
        peaks = []
        decays = []
        for c in range(channels):
            amp = float(np.max(np.abs(frames[:, c]))) if len(frames) else 0.0
            peak = 20.0 * math.log10(amp) if amp > 0 else SILENCE_DB
            self.peak_age[c] += dt
            if peak >= self.decay_db[c]:
                self.decay_db[c] = peak
                self.peak_age[c] = 0.0
            elif self.peak_age[c] > PEAK_TTL:
                falloff = PEAK_FALLOFF * min(dt, self.peak_age[c] - PEAK_TTL)
                self.decay_db[c] = max(peak, self.decay_db[c] - falloff)
            peaks.append(peak)
            decays.append(self.decay_db[c])
        return peaks, decays


    def on_frames(self, frames):
        if self.vumeter:
            peaks, decays = self.compute_level(frames)
            magnitudes = []
            for channel in range(0, min(self.bands, len(peaks))):
                peak = max(-self.threshold, min(0, peaks[channel]))
                decay = max(-self.threshold, min(0, decays[channel]))
                if not self.db:
                    if self.logamplify:
                        peak = self.dbtopct(peak, peak)
                        decay = self.dbtopct(decay, decay)
                    else:
                        peak = self.dbtopct(peak)
                        decay = self.dbtopct(decay)
                magnitudes.append(self.round(peak))
                magnitudes.append(self.round(decay))
        else:
            magnitudes = self.compute_spectrum(frames)
            if magnitudes is None:
                return
            magnitudes = magnitudes[:self.bands_cutoff]
            if not self.db:
                if self.logamplify:
                    magnitudes = [self.dbtopct(db, i) for i, db
                                  in enumerate(magnitudes)]
                else:
                    magnitudes = [self.dbtopct(db) for db in magnitudes]
            if not self.raw:
                magnitudes = self.scale(magnitudes, self.bands)
            magnitudes = [self.round(m) for m in magnitudes]

        if not self.quiet:
            print(' | '.join(('%.3f' % m for m in magnitudes)))

        if self.callback:
            self.callback(magnitudes)


    def capture_loop(self):
        _init_com()
        frames_per_interval = max(1, int(SAMPLERATE * self.interval / 1000.0))
        try:
            with self.device.recorder(samplerate=SAMPLERATE, channels=2) as recorder:
                while self.running:
                    frames = recorder.record(numframes=frames_per_interval)
                    if not self.running:
                        break
                    self.on_frames(frames)
        except Exception as exc: # pylint: disable=broad-except
            stderr('Audio capture failed: {}'.format(exc))
            self.running = False


    def start(self):
        if not self.quiet:
            stdout('Capturing audio from: {}'.format(self.device.name))
        self.running = True
        self.thread = Thread(target=self.capture_loop)
        self.thread.daemon = True
        self.thread.start()


    def stop(self):
        self.running = False
        if (self.thread is not None and self.thread.is_alive()
                and self.thread is not current_thread()):
            self.thread.join(2)
