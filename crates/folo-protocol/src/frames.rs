//! Surface and signal frames on the source link (RB-08): the little-endian
//! `FOLS` and `FOLG` layouts of [the source link
//! specification](../../../docs/specs/source-link.md#surface-frames), the same as
//! `source/folo_source/publish/frames.py` writes and `specs/asyncapi.yaml` lists.
//!
//! The controller doesn't decompress: a surface frame with the LZ4 flag is an
//! error ([`FrameError::Lz4Unsupported`]) and the caller drops it with a log
//! line, until RB-16 adds compression. Nothing here ever writes the flag.

use std::fmt;

/// Frame format version 1, the only one there is.
pub const FORMAT_VERSION: u8 = 1;
/// The first four bytes of a surface frame.
pub const SURFACE_MAGIC: [u8; 4] = *b"FOLS";
/// The first four bytes of a signal frame.
pub const SIGNAL_MAGIC: [u8; 4] = *b"FOLG";
/// Flag bit 0: the payload is an LZ4 frame.
pub const FLAG_LZ4: u8 = 0b01;
/// Flag bit 1: a key frame. Version 1 has no delta frames, so every frame is one.
pub const FLAG_KEY_FRAME: u8 = 0b10;
/// Bytes before a surface frame's payload.
pub const SURFACE_HEADER_LEN: usize = 28;
/// Bytes before a signal frame's values.
pub const SIGNAL_HEADER_LEN: usize = 24;
/// Channels of a cell: red, green and blue, in that order.
pub const CHANNELS: usize = 3;

/// `rgb16` is `⌊value × 257⌋`, so 255 becomes 65,535.
const RGB16_SCALE: f32 = 257.0;

/// A frame that doesn't follow the source link's layout.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FrameError {
    /// Shorter than the header.
    TooShort,
    /// Not `FOLS` (or `FOLG`).
    BadMagic,
    /// A format version other than 1.
    BadVersion(u8),
    /// A pixel format code other than 1 (rgb8), 2 (rgb16) or 3 (rgbf32).
    BadPixelFormat(u8),
    /// The LZ4 flag is set, and the controller doesn't decompress (RB-16).
    Lz4Unsupported,
    /// The payload isn't the size the header says.
    BadPayloadSize {
        /// Bytes the header's size, format and count call for.
        expected: usize,
        /// Bytes that are there.
        actual: usize,
    },
}

impl fmt::Display for FrameError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::TooShort => f.write_str("shorter than the frame header"),
            Self::BadMagic => f.write_str("wrong magic bytes"),
            Self::BadVersion(version) => {
                write!(f, "frame format version {version}, not {FORMAT_VERSION}")
            }
            Self::BadPixelFormat(code) => write!(f, "unknown pixel format {code}"),
            Self::Lz4Unsupported => f.write_str("LZ4 frames aren't supported yet"),
            Self::BadPayloadSize { expected, actual } => {
                write!(f, "payload is {actual} bytes, not {expected}")
            }
        }
    }
}

impl std::error::Error for FrameError {}

/// How a surface frame's cells are stored.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PixelFormat {
    /// One byte per channel, 0–255.
    Rgb8,
    /// Two bytes per channel, 0–65,535 for 0–255.
    Rgb16,
    /// A float32 per channel on the 0–255 scale.
    RgbF32,
}

impl PixelFormat {
    /// The code in the header.
    #[must_use]
    pub fn code(self) -> u8 {
        match self {
            Self::Rgb8 => 1,
            Self::Rgb16 => 2,
            Self::RgbF32 => 3,
        }
    }

    /// The format a header code names.
    #[must_use]
    pub fn from_code(code: u8) -> Option<Self> {
        match code {
            1 => Some(Self::Rgb8),
            2 => Some(Self::Rgb16),
            3 => Some(Self::RgbF32),
            _ => None,
        }
    }

    /// Bytes per channel.
    #[must_use]
    pub fn bytes_per_channel(self) -> usize {
        match self {
            Self::Rgb8 => 1,
            Self::Rgb16 => 2,
            Self::RgbF32 => 4,
        }
    }
}

/// A surface frame's cells, row-major, red then green then blue, in the
/// frame's own type.
#[derive(Clone, Debug, PartialEq)]
pub enum Pixels {
    /// `rgb8`.
    Rgb8(Vec<u8>),
    /// `rgb16`.
    Rgb16(Vec<u16>),
    /// `rgbf32`.
    RgbF32(Vec<f32>),
}

impl Pixels {
    /// The format of these cells.
    #[must_use]
    pub fn format(&self) -> PixelFormat {
        match self {
            Self::Rgb8(_) => PixelFormat::Rgb8,
            Self::Rgb16(_) => PixelFormat::Rgb16,
            Self::RgbF32(_) => PixelFormat::RgbF32,
        }
    }

    /// How many channel values there are (cells × 3).
    #[must_use]
    pub fn len(&self) -> usize {
        match self {
            Self::Rgb8(values) => values.len(),
            Self::Rgb16(values) => values.len(),
            Self::RgbF32(values) => values.len(),
        }
    }

    /// Whether there are no values.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

/// A decoded surface frame (`FOLS`).
#[derive(Clone, Debug, PartialEq)]
pub struct SurfaceFrame {
    /// The surface index, as assigned in `config.apply`.
    pub index: u16,
    /// The header's flags. A decoded frame never has the LZ4 bit.
    pub flags: u8,
    /// Counts the frames sent for the surface, wrapping at 2³².
    pub sequence: u32,
    /// The source's monotonic clock, µs.
    pub timestamp_us: u64,
    /// Cells across.
    pub width: u16,
    /// Cells down (1 for strips).
    pub height: u16,
    /// The cells.
    pub pixels: Pixels,
}

impl SurfaceFrame {
    /// A key frame of `pixels`.
    #[must_use]
    pub fn new(
        index: u16,
        sequence: u32,
        timestamp_us: u64,
        width: u16,
        height: u16,
        pixels: Pixels,
    ) -> Self {
        Self {
            index,
            flags: FLAG_KEY_FRAME,
            sequence,
            timestamp_us,
            width,
            height,
            pixels,
        }
    }

    /// How many cells the header's size has.
    #[must_use]
    pub fn cells(&self) -> usize {
        usize::from(self.width) * usize::from(self.height)
    }

    /// The channel values on the engine's 0–255 scale, as floats: `rgb8`
    /// exactly, `rgb16` divided by 257 (the inverse of how it was made) and
    /// `rgbf32` as it is.
    #[must_use]
    pub fn to_f32(&self) -> Vec<f32> {
        match &self.pixels {
            Pixels::Rgb8(values) => values.iter().map(|&v| f32::from(v)).collect(),
            Pixels::Rgb16(values) => values.iter().map(|&v| f32::from(v) / RGB16_SCALE).collect(),
            Pixels::RgbF32(values) => values.clone(),
        }
    }
}

/// A decoded signal frame (`FOLG`).
#[derive(Clone, Debug, PartialEq)]
pub struct SignalFrame {
    /// The signal index.
    pub index: u16,
    /// Counts the frames sent for the signal, wrapping at 2³².
    pub sequence: u32,
    /// The source's monotonic clock, µs.
    pub timestamp_us: u64,
    /// The values.
    pub values: Vec<f32>,
}

fn le_u16(data: &[u8], at: usize) -> u16 {
    u16::from_le_bytes([data[at], data[at + 1]])
}

fn le_u32(data: &[u8], at: usize) -> u32 {
    u32::from_le_bytes([data[at], data[at + 1], data[at + 2], data[at + 3]])
}

fn le_u64(data: &[u8], at: usize) -> u64 {
    let mut bytes = [0; 8];
    bytes.copy_from_slice(&data[at..at + 8]);
    u64::from_le_bytes(bytes)
}

/// Reads a surface frame. The reserved bytes aren't checked, as Python's
/// decoder doesn't.
///
/// # Errors
/// A [`FrameError`] when the bytes aren't a version 1 surface frame, when the
/// LZ4 flag is set, or when the payload isn't `width × height × 3` values.
pub fn decode_surface(data: &[u8]) -> Result<SurfaceFrame, FrameError> {
    if data.len() < SURFACE_HEADER_LEN {
        return Err(FrameError::TooShort);
    }
    if data[..4] != SURFACE_MAGIC {
        return Err(FrameError::BadMagic);
    }
    if data[4] != FORMAT_VERSION {
        return Err(FrameError::BadVersion(data[4]));
    }
    let format = PixelFormat::from_code(data[5]).ok_or(FrameError::BadPixelFormat(data[5]))?;
    let flags = data[6];
    if flags & FLAG_LZ4 != 0 {
        return Err(FrameError::Lz4Unsupported);
    }
    let (index, width, height) = (le_u16(data, 8), le_u16(data, 10), le_u16(data, 12));
    let sequence = le_u32(data, 16);
    let timestamp_us = le_u64(data, 20);
    let payload = &data[SURFACE_HEADER_LEN..];
    let values = usize::from(width) * usize::from(height) * CHANNELS;
    let expected = values * format.bytes_per_channel();
    if payload.len() != expected {
        return Err(FrameError::BadPayloadSize {
            expected,
            actual: payload.len(),
        });
    }
    let pixels = match format {
        PixelFormat::Rgb8 => Pixels::Rgb8(payload.to_vec()),
        PixelFormat::Rgb16 => Pixels::Rgb16(
            payload
                .as_chunks::<2>()
                .0
                .iter()
                .map(|c| u16::from_le_bytes(*c))
                .collect(),
        ),
        PixelFormat::RgbF32 => Pixels::RgbF32(
            payload
                .as_chunks::<4>()
                .0
                .iter()
                .map(|c| f32::from_le_bytes(*c))
                .collect(),
        ),
    };
    Ok(SurfaceFrame {
        index,
        flags,
        sequence,
        timestamp_us,
        width,
        height,
        pixels,
    })
}

/// Writes a surface frame, never compressed (the LZ4 bit is cleared and the
/// key frame bit set, as version 1 has no delta frames).
///
/// # Errors
/// [`FrameError::BadPayloadSize`] when `frame.pixels` isn't
/// `width × height × 3` values.
pub fn encode_surface(frame: &SurfaceFrame) -> Result<Vec<u8>, FrameError> {
    let values = frame.cells() * CHANNELS;
    if frame.pixels.len() != values {
        return Err(FrameError::BadPayloadSize {
            expected: values,
            actual: frame.pixels.len(),
        });
    }
    let format = frame.pixels.format();
    let mut out = Vec::with_capacity(SURFACE_HEADER_LEN + values * format.bytes_per_channel());
    out.extend_from_slice(&SURFACE_MAGIC);
    out.push(FORMAT_VERSION);
    out.push(format.code());
    out.push((frame.flags & !FLAG_LZ4) | FLAG_KEY_FRAME);
    out.push(0);
    out.extend_from_slice(&frame.index.to_le_bytes());
    out.extend_from_slice(&frame.width.to_le_bytes());
    out.extend_from_slice(&frame.height.to_le_bytes());
    out.extend_from_slice(&[0, 0]);
    out.extend_from_slice(&frame.sequence.to_le_bytes());
    out.extend_from_slice(&frame.timestamp_us.to_le_bytes());
    match &frame.pixels {
        Pixels::Rgb8(values) => out.extend_from_slice(values),
        Pixels::Rgb16(values) => values
            .iter()
            .for_each(|v| out.extend_from_slice(&v.to_le_bytes())),
        Pixels::RgbF32(values) => values
            .iter()
            .for_each(|v| out.extend_from_slice(&v.to_le_bytes())),
    }
    Ok(out)
}

/// Reads a signal frame.
///
/// # Errors
/// A [`FrameError`] when the bytes aren't a version 1 signal frame or the
/// payload isn't `4 × count` bytes.
pub fn decode_signal(data: &[u8]) -> Result<SignalFrame, FrameError> {
    if data.len() < SIGNAL_HEADER_LEN {
        return Err(FrameError::TooShort);
    }
    if data[..4] != SIGNAL_MAGIC {
        return Err(FrameError::BadMagic);
    }
    if data[4] != FORMAT_VERSION {
        return Err(FrameError::BadVersion(data[4]));
    }
    let index = le_u16(data, 6);
    let count = usize::from(le_u16(data, 8));
    let sequence = le_u32(data, 12);
    let timestamp_us = le_u64(data, 16);
    let payload = &data[SIGNAL_HEADER_LEN..];
    if payload.len() != 4 * count {
        return Err(FrameError::BadPayloadSize {
            expected: 4 * count,
            actual: payload.len(),
        });
    }
    let values = payload
        .as_chunks::<4>()
        .0
        .iter()
        .map(|c| f32::from_le_bytes(*c))
        .collect();
    Ok(SignalFrame {
        index,
        sequence,
        timestamp_us,
        values,
    })
}

/// Writes a signal frame.
///
/// # Errors
/// [`FrameError::BadPayloadSize`] when there are more values than the
/// header's 16-bit count holds.
pub fn encode_signal(frame: &SignalFrame) -> Result<Vec<u8>, FrameError> {
    let count = u16::try_from(frame.values.len()).map_err(|_| FrameError::BadPayloadSize {
        expected: usize::from(u16::MAX),
        actual: frame.values.len(),
    })?;
    let mut out = Vec::with_capacity(SIGNAL_HEADER_LEN + 4 * frame.values.len());
    out.extend_from_slice(&SIGNAL_MAGIC);
    out.push(FORMAT_VERSION);
    out.push(0);
    out.extend_from_slice(&frame.index.to_le_bytes());
    out.extend_from_slice(&count.to_le_bytes());
    out.extend_from_slice(&[0, 0]);
    out.extend_from_slice(&frame.sequence.to_le_bytes());
    out.extend_from_slice(&frame.timestamp_us.to_le_bytes());
    for value in &frame.values {
        out.extend_from_slice(&value.to_le_bytes());
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SURFACE_FIXTURE: &[u8] =
        include_bytes!("../../../specs/fixtures/surfaceFrame--rgb8.fols");
    const SIGNAL_FIXTURE: &[u8] = include_bytes!("../../../specs/fixtures/signalFrame--beat.folg");

    fn strip(pixels: Pixels, width: u16) -> SurfaceFrame {
        SurfaceFrame::new(7, 3, 123_456, width, 1, pixels)
    }

    #[test]
    fn the_surface_fixture_decodes_and_encodes_to_the_same_bytes() {
        let frame = decode_surface(SURFACE_FIXTURE).unwrap();
        assert_eq!(
            (
                frame.index,
                frame.width,
                frame.height,
                frame.sequence,
                frame.timestamp_us
            ),
            (0, 4, 1, 42, 1_234_567)
        );
        assert_eq!(frame.flags, FLAG_KEY_FRAME);
        assert_eq!(
            frame.pixels,
            Pixels::Rgb8(vec![
                0x4b, 0x00, 0xff, 0xff, 0x33, 0x99, 0x00, 0x33, 0xcc, 0, 0, 0
            ])
        );
        assert_eq!(encode_surface(&frame).unwrap(), SURFACE_FIXTURE);
    }

    #[test]
    fn the_signal_fixture_decodes_and_encodes_to_the_same_bytes() {
        let frame = decode_signal(SIGNAL_FIXTURE).unwrap();
        assert_eq!(
            (frame.index, frame.sequence, frame.timestamp_us),
            (0, 42, 1_234_567)
        );
        assert_eq!(frame.values, vec![120.0, 0.7, 1.0, 0.25, 1.0]);
        assert_eq!(encode_signal(&frame).unwrap(), SIGNAL_FIXTURE);
    }

    /// Frames `frames.surface_frame(7, pixels, fmt, 3, 123_456, False)` wrote
    /// for `pixels = [[200.5, 0, 255], [1, 128.9, 64]]` (float32), and
    /// `frames.signal_frame(5, (1.5, -2.0, 0.25), 9, 77)`.
    const PYTHON_RGB8: &str =
        "464f4c530101020007000200010000000300000040e2010000000000        c800ff018040";
    const PYTHON_RGB16: &str =
        "464f4c530102020007000200010000000300000040e2010000000000        48c90000ffff010167814040";
    const PYTHON_RGBF32: &str = "464f4c530103020007000200010000000300000040e2010000000000        008048430000000000007f430000803f66e6004300008042";
    const PYTHON_SIGNAL: &str =
        "464f4c470100050003000000090000004d00000000000000        0000c03f000000c00000803e";

    fn hex(text: &str) -> Vec<u8> {
        let text: String = text.split_whitespace().collect();
        (0..text.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&text[i..i + 2], 16).unwrap())
            .collect()
    }

    #[test]
    fn rgb8_rgb16_and_rgbf32_round_trip() {
        let frames = [
            strip(Pixels::Rgb8(vec![1, 2, 3, 250, 251, 255]), 2),
            strip(Pixels::Rgb16(vec![0, 257, 65_535, 12_345, 1, 2]), 2),
            strip(Pixels::RgbF32(vec![0.0, 1.5, 255.0, 0.1, 99.9, 3.0]), 2),
        ];
        for frame in frames {
            let bytes = encode_surface(&frame).unwrap();
            assert_eq!(decode_surface(&bytes).unwrap(), frame);
        }
    }

    #[test]
    fn frames_written_by_the_python_publisher_decode_and_encode_to_the_same_bytes() {
        let expected = [
            (PYTHON_RGB8, Pixels::Rgb8(vec![200, 0, 255, 1, 128, 64])),
            (
                PYTHON_RGB16,
                Pixels::Rgb16(vec![51_528, 0, 65_535, 257, 33_127, 16_448]),
            ),
            (
                PYTHON_RGBF32,
                Pixels::RgbF32(vec![200.5, 0.0, 255.0, 1.0, 128.9, 64.0]),
            ),
        ];
        for (text, pixels) in expected {
            let bytes = hex(text);
            let frame = decode_surface(&bytes).unwrap();
            assert_eq!(
                (
                    frame.index,
                    frame.sequence,
                    frame.timestamp_us,
                    frame.width,
                    frame.height
                ),
                (7, 3, 123_456, 2, 1)
            );
            assert_eq!(frame.pixels, pixels);
            assert_eq!(encode_surface(&frame).unwrap(), bytes);
        }
        let signal = decode_signal(&hex(PYTHON_SIGNAL)).unwrap();
        assert_eq!(
            (signal.index, signal.sequence, signal.timestamp_us),
            (5, 9, 77)
        );
        assert_eq!(signal.values, vec![1.5, -2.0, 0.25]);
        assert_eq!(encode_signal(&signal).unwrap(), hex(PYTHON_SIGNAL));
    }

    #[test]
    fn rgb16_converts_back_to_the_engines_scale() {
        let frame = strip(Pixels::Rgb16(vec![0, 257, 65_535, 51_528, 1, 258]), 2);
        let values = frame.to_f32();
        assert_eq!(&values[..3], &[0.0, 1.0, 255.0]);
        assert_eq!(values[3], 51_528.0 / 257.0);
        assert_eq!(values[5], 258.0 / 257.0);
    }

    #[test]
    fn lz4_frames_are_refused() {
        let mut bytes = SURFACE_FIXTURE.to_vec();
        bytes[6] |= FLAG_LZ4;
        assert_eq!(decode_surface(&bytes), Err(FrameError::Lz4Unsupported));
    }

    #[test]
    fn the_encoder_never_writes_the_lz4_bit() {
        let mut frame = strip(Pixels::Rgb8(vec![0; 6]), 2);
        frame.flags = FLAG_LZ4;
        let bytes = encode_surface(&frame).unwrap();
        assert_eq!(bytes[6], FLAG_KEY_FRAME);
    }

    #[test]
    fn bad_surface_frames_are_refused() {
        let mut wrong_version = SURFACE_FIXTURE.to_vec();
        wrong_version[4] = 2;
        let mut wrong_format = SURFACE_FIXTURE.to_vec();
        wrong_format[5] = 9;
        let cases: [(&[u8], FrameError); 6] = [
            (b"", FrameError::TooShort),
            (b"FOLS", FrameError::TooShort),
            (&[b'X'; 28], FrameError::BadMagic),
            (&wrong_version, FrameError::BadVersion(2)),
            (&wrong_format, FrameError::BadPixelFormat(9)),
            (
                &SURFACE_FIXTURE[..SURFACE_FIXTURE.len() - 1],
                FrameError::BadPayloadSize {
                    expected: 12,
                    actual: 11,
                },
            ),
        ];
        for (bytes, error) in cases {
            assert_eq!(decode_surface(bytes), Err(error));
        }
        let mut long = SURFACE_FIXTURE.to_vec();
        long.push(0);
        assert!(matches!(
            decode_surface(&long),
            Err(FrameError::BadPayloadSize { .. })
        ));
    }

    #[test]
    fn bad_signal_frames_are_refused() {
        assert_eq!(
            decode_signal(&SIGNAL_FIXTURE[..10]),
            Err(FrameError::TooShort)
        );
        assert_eq!(decode_signal(SURFACE_FIXTURE), Err(FrameError::BadMagic));
        assert_eq!(
            decode_signal(&SIGNAL_FIXTURE[..SIGNAL_FIXTURE.len() - 1]),
            Err(FrameError::BadPayloadSize {
                expected: 20,
                actual: 19
            })
        );
    }

    #[test]
    fn a_frame_whose_pixels_dont_fit_its_size_isnt_encoded() {
        let frame = strip(Pixels::Rgb8(vec![0; 5]), 2);
        assert!(encode_surface(&frame).is_err());
    }

    #[test]
    fn the_largest_surface_frame_fits_the_header() {
        let frame = SurfaceFrame::new(
            u16::MAX,
            u32::MAX,
            u64::MAX,
            2000,
            1,
            Pixels::Rgb8(vec![9; 6000]),
        );
        let decoded = decode_surface(&encode_surface(&frame).unwrap()).unwrap();
        assert_eq!(decoded, frame);
    }
}
