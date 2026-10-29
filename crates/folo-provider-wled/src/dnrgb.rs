//! DNRGB and DNRGBW packets, WLED's UDP realtime protocols 4 and 5 on port 21324
//! (`docs/integrations/wled.md`, realtime protocols).
//!
//! A packet is the protocol (4 or 5), the timeout in seconds (0 leaves realtime at once, 255
//! never times out), the start index (u16, big-endian) and the pixels. WLED renders on every
//! packet, so a frame in several packets can tear.

use crate::ddp::PixelFormat;

/// The UDP port of WLED's UDP realtime, unless the device was set to another.
pub const DNRGB_PORT: u16 = 21324;

/// The most pixels in one packet: what fits in 1472 bytes after the four of the header.
const MAX_RGB_PIXELS: usize = 489;
const MAX_RGBW_PIXELS: usize = 367;

/// "Never times out": a device whose controller died would stay in realtime for good, so the
/// timeout FOLO sends stops short of it.
pub const MAX_TIMEOUT_S: u8 = 254;

const PROTOCOL_DNRGB: u8 = 4;
const PROTOCOL_DNRGBW: u8 = 5;

fn protocol(format: PixelFormat) -> u8 {
    match format {
        PixelFormat::Rgb => PROTOCOL_DNRGB,
        PixelFormat::Rgbw => PROTOCOL_DNRGBW,
    }
}

fn max_pixels(format: PixelFormat) -> usize {
    match format {
        PixelFormat::Rgb => MAX_RGB_PIXELS,
        PixelFormat::Rgbw => MAX_RGBW_PIXELS,
    }
}

/// How many pixels each packet of `total` carries. WLED ignores a packet of one pixel (it wants
/// more than seven bytes), so a lone pixel at the end is paired with the one before it.
fn chunk_sizes(total: usize, max: usize) -> Vec<usize> {
    let mut sizes = vec![max; total / max];
    let rest = total % max;
    if rest == 1 && !sizes.is_empty() {
        let last = sizes.len() - 1;
        sizes[last] -= 1;
        sizes.push(2);
    } else if rest > 0 {
        sizes.push(rest);
    }
    sizes
}

/// Makes DNRGB(W) packets with the timeout byte it was given.
#[derive(Clone, Copy, Debug)]
pub struct DnrgbEncoder {
    timeout_s: u8,
}

impl DnrgbEncoder {
    /// `timeout_s` is capped at [`MAX_TIMEOUT_S`] and raised to 1 (0 would end realtime).
    #[must_use]
    pub fn new(timeout_s: u64) -> Self {
        let capped = timeout_s.clamp(1, u64::from(MAX_TIMEOUT_S));
        Self {
            timeout_s: u8::try_from(capped).unwrap_or(MAX_TIMEOUT_S),
        }
    }

    #[must_use]
    pub fn timeout_s(self) -> u8 {
        self.timeout_s
    }

    /// The packets that write `data` (whole pixels of `format`) from `first_pixel` on. Pixels
    /// beyond index 65535 can't be addressed and are dropped. A single pixel can't be sent: WLED
    /// ignores a packet of one, and a copy on the next LED would overwrite it, so there are none
    /// (the provider sends those by DDP).
    #[must_use]
    #[allow(clippy::cast_possible_truncation)]
    pub fn pixels(self, format: PixelFormat, first_pixel: u32, data: &[u8]) -> Vec<Vec<u8>> {
        let pixel = format.bytes();
        let mut data = data[..data.len() / pixel * pixel].to_vec();
        let reachable = (u32::from(u16::MAX) + 1).saturating_sub(first_pixel) as usize;
        data.truncate(reachable * pixel);
        if data.len() <= pixel {
            return Vec::new();
        }
        let mut packets = Vec::new();
        let mut index = first_pixel;
        let mut rest = data.as_slice();
        for size in chunk_sizes(data.len() / pixel, max_pixels(format)) {
            let (chunk, tail) = rest.split_at(size * pixel);
            packets.push(self.packet(format, index, chunk));
            index += size as u32;
            rest = tail;
        }
        packets
    }

    #[allow(clippy::cast_possible_truncation)]
    fn packet(self, format: PixelFormat, start: u32, data: &[u8]) -> Vec<u8> {
        let mut out = Vec::with_capacity(4 + data.len());
        out.push(protocol(format));
        out.push(self.timeout_s);
        out.extend((start as u16).to_be_bytes());
        out.extend(data);
        out
    }

    /// The packet that gives the lights back at once: timeout 0.
    #[must_use]
    pub fn release(self) -> Vec<u8> {
        vec![PROTOCOL_DNRGB, 0, 0, 0]
    }
}
