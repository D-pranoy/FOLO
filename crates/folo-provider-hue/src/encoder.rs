//! `HueStream` v2 packets (`docs/integrations/hue.md`, packets and encoding).
//!
//! ```text
//! "HueStream" 02 00 <sequence> 00 00 <colour space> 00 <area id, 36 ASCII> { <channel id> <R16> <G16> <B16> }*
//! ```
//!
//! Colours are RGB (`00`). Every packet carries every channel of the area, because bridges lose
//! sync otherwise, and at most 20 of them. The sequence counts up modulo 256.

use std::fmt;

const MAGIC: &[u8; 9] = b"HueStream";
const VERSION: [u8; 2] = [0x02, 0x00];
const COLOR_SPACE_RGB: u8 = 0x00;
/// The most channels an entertainment area (and so a packet) has.
pub const MAX_CHANNELS: usize = 20;
/// The length of an area id: a UUID as lowercase text.
const AREA_ID_LENGTH: usize = 36;
const HEADER_LENGTH: usize = 16 + AREA_ID_LENGTH;
const RECORD_LENGTH: usize = 7;

/// A packet can't be made.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum EncodeError {
    /// The area id isn't a 36 character UUID.
    BadAreaId(String),
    /// More channels than a packet takes.
    TooManyChannels(usize),
}

impl fmt::Display for EncodeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::BadAreaId(id) => write!(f, "{id:?} is not an entertainment area id"),
            Self::TooManyChannels(n) => write!(
                f,
                "the area has {n} channels and a stream carries at most {MAX_CHANNELS}"
            ),
        }
    }
}

impl std::error::Error for EncodeError {}

/// Makes the packets of one stream, numbering them.
#[derive(Clone, Debug)]
pub struct Encoder {
    area_id: [u8; AREA_ID_LENGTH],
    sequence: u8,
}

/// An 8-bit level as the bridge's 16 bits, by byte duplication: 255 becomes `0xFFFF`.
#[must_use]
pub fn widen(level: u8) -> u16 {
    u16::from_be_bytes([level, level])
}

impl Encoder {
    /// An encoder for the area with this id (a UUID; upper case is written as lower case).
    ///
    /// # Errors
    /// [`EncodeError::BadAreaId`] when it isn't 36 ASCII characters.
    pub fn new(area_id: &str) -> Result<Self, EncodeError> {
        let lower = area_id.to_ascii_lowercase();
        let bytes: [u8; AREA_ID_LENGTH] = lower
            .as_bytes()
            .try_into()
            .map_err(|_| EncodeError::BadAreaId(area_id.to_owned()))?;
        Ok(Self {
            area_id: bytes,
            sequence: 0,
        })
    }

    /// The packet for the next frame: `channels` are channel ids with their colours, 0–255.
    ///
    /// # Errors
    /// [`EncodeError::TooManyChannels`] for more than [`MAX_CHANNELS`].
    pub fn encode(&mut self, channels: &[(u8, [u8; 3])]) -> Result<Vec<u8>, EncodeError> {
        if channels.len() > MAX_CHANNELS {
            return Err(EncodeError::TooManyChannels(channels.len()));
        }
        let mut packet = Vec::with_capacity(HEADER_LENGTH + RECORD_LENGTH * channels.len());
        packet.extend_from_slice(MAGIC);
        packet.extend_from_slice(&VERSION);
        packet.push(self.sequence);
        packet.extend_from_slice(&[0x00, 0x00, COLOR_SPACE_RGB, 0x00]);
        packet.extend_from_slice(&self.area_id);
        for (id, rgb) in channels {
            packet.push(*id);
            for level in rgb {
                packet.extend_from_slice(&widen(*level).to_be_bytes());
            }
        }
        self.sequence = self.sequence.wrapping_add(1);
        Ok(packet)
    }

    /// The sequence number the next packet gets.
    #[must_use]
    pub fn next_sequence(&self) -> u8 {
        self.sequence
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const AREA: &str = "0a1b2c3d-4e5f-6071-8293-a4b5c6d7e8f9";

    /// The bytes a bridge is sent for two channels, written out by hand from the layout above.
    #[test]
    fn golden_packet_for_two_channels() {
        let mut encoder = Encoder::new(AREA).unwrap();
        let packet = encoder
            .encode(&[(0, [255, 0, 128]), (3, [1, 2, 3])])
            .unwrap();
        let expected = concat!(
            "487565 53747265 616d", // "HueStream"
            "0200",                 // version 2.0
            "00",                   // sequence
            "0000",                 // reserved
            "00",                   // RGB
            "00",                   // reserved
            "30613162326333642d346535662d363037312d383239332d613462356336643765386639", // the area id
            "00 ffff 0000 8080", // channel 0: 255, 0, 128
            "03 0101 0202 0303", // channel 3: 1, 2, 3
        )
        .replace(' ', "");
        assert_eq!(hex::encode(&packet), expected);
    }

    #[test]
    fn the_sequence_counts_modulo_256() {
        let mut encoder = Encoder::new(AREA).unwrap();
        let sequences: Vec<u8> = (0..258)
            .map(|_| encoder.encode(&[(0, [0, 0, 0])]).unwrap()[11])
            .collect();
        assert_eq!(&sequences[..3], &[0, 1, 2]);
        assert_eq!(sequences[255], 255);
        assert_eq!(&sequences[256..], &[0, 1]);
    }

    #[test]
    fn bytes_are_widened_by_duplication() {
        assert_eq!(widen(0), 0x0000);
        assert_eq!(widen(0x7f), 0x7f7f);
        assert_eq!(widen(255), 0xffff);
    }

    #[test]
    fn twenty_channels_fit_and_twenty_one_do_not() {
        let mut encoder = Encoder::new(AREA).unwrap();
        let channels: Vec<(u8, [u8; 3])> = (0..20).map(|i| (i, [i; 3])).collect();
        assert_eq!(encoder.encode(&channels).unwrap().len(), 52 + 20 * 7);
        let too_many: Vec<(u8, [u8; 3])> = (0..21).map(|i| (i, [0; 3])).collect();
        assert_eq!(
            encoder.encode(&too_many),
            Err(EncodeError::TooManyChannels(21))
        );
    }

    #[test]
    fn the_area_id_is_lowercased_and_checked() {
        let upper = Encoder::new(&AREA.to_ascii_uppercase()).unwrap();
        assert_eq!(upper.area_id, Encoder::new(AREA).unwrap().area_id);
        assert!(Encoder::new("not-a-uuid").is_err());
    }
}
