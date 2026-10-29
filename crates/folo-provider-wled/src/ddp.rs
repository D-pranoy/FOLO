//! DDP packets (`docs/integrations/wled.md`, realtime protocols).
//!
//! A packet is a 10-byte header and at most 1440 data bytes (480 RGB or 360 RGBW pixels).
//! The header is the flags (`0x40` for version 1, plus push `0x01` on the last packet of a
//! frame), the sequence number (1–15), the data type (`0x0B` RGB, `0x1B` RGBW), the destination
//! id (1), the byte offset (u32, big-endian) and the length (u16, big-endian).

/// The UDP port WLED listens on for DDP.
pub const DDP_PORT: u16 = 4048;

/// The most data bytes in one packet. It is a whole number of pixels in both formats.
pub const MAX_DATA_BYTES: usize = 1440;

/// The bytes before the data.
pub const HEADER_BYTES: usize = 10;

const FLAGS_VERSION_1: u8 = 0x40;
const FLAG_PUSH: u8 = 0x01;
const DESTINATION_DEFAULT: u8 = 1;
const SEQUENCE_MAX: u8 = 15;

/// How a pixel is laid out on the wire.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PixelFormat {
    Rgb,
    Rgbw,
}

impl PixelFormat {
    /// Bytes per pixel.
    #[must_use]
    pub fn bytes(self) -> usize {
        match self {
            Self::Rgb => 3,
            Self::Rgbw => 4,
        }
    }

    /// DDP's data type byte.
    #[must_use]
    pub fn data_type(self) -> u8 {
        match self {
            Self::Rgb => 0x0B,
            Self::Rgbw => 0x1B,
        }
    }
}

/// Pixels to write: `data` holds whole pixels of `format`, the first one at `first_pixel`.
#[derive(Clone, Copy, Debug)]
pub struct PixelRun<'a> {
    pub format: PixelFormat,
    pub first_pixel: u32,
    pub data: &'a [u8],
}

/// Makes the packets of a frame. It owns the sequence number, which counts packets and wraps
/// from 15 to 1 (0 means "unused" to the receiver).
#[derive(Debug)]
pub struct DdpEncoder {
    sequence: u8,
}

impl Default for DdpEncoder {
    fn default() -> Self {
        Self::new()
    }
}

impl DdpEncoder {
    #[must_use]
    pub fn new() -> Self {
        Self { sequence: 0 }
    }

    fn next_sequence(&mut self) -> u8 {
        self.sequence = if self.sequence >= SEQUENCE_MAX {
            1
        } else {
            self.sequence + 1
        };
        self.sequence
    }

    /// The packets of one frame, in order. Only the last has the push flag, so WLED renders
    /// the frame once, whole. A run's trailing bytes that don't make a whole pixel are dropped.
    pub fn frame(&mut self, runs: &[PixelRun<'_>]) -> Vec<Vec<u8>> {
        let mut packets = Vec::new();
        for run in runs {
            let pixel = run.format.bytes();
            let whole = run.data.len() / pixel * pixel;
            for (index, chunk) in run.data[..whole].chunks(MAX_DATA_BYTES).enumerate() {
                let byte_offset =
                    u64::from(run.first_pixel) * pixel as u64 + (index * MAX_DATA_BYTES) as u64;
                let sequence = self.next_sequence();
                packets.push(packet(run.format, sequence, byte_offset, chunk));
            }
        }
        if let Some(last) = packets.last_mut() {
            last[0] |= FLAG_PUSH;
        }
        packets
    }
}

#[allow(clippy::cast_possible_truncation)]
fn packet(format: PixelFormat, sequence: u8, byte_offset: u64, data: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(HEADER_BYTES + data.len());
    out.push(FLAGS_VERSION_1);
    out.push(sequence);
    out.push(format.data_type());
    out.push(DESTINATION_DEFAULT);
    out.extend((byte_offset as u32).to_be_bytes());
    out.extend((data.len() as u16).to_be_bytes());
    out.extend(data);
    out
}
