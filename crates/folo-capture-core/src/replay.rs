//! The replay backend (ADR-0037 D22): plays back a recorded sequence of
//! frames, so the agent, the node and the engine are tested with the same
//! pictures every time. Replay files are synthetic; recordings of real
//! screens are never committed (docs/integrations/display-capture.md#latency-and-privacy).
//!
//! The file format is documented in docs/integrations/display-capture.md#replay-files:
//! the 8-byte header `FOLR`, version 1 and three zero bytes, then records of a
//! `u32` little-endian length and that many bytes of a `FOLC` frame.

use std::io::Read as _;
use std::path::Path;
use std::thread;
use std::time::{Duration, Instant};

use crate::backend::CaptureBackend;
use crate::codec::{self, HEADER_LEN, MAX_MESSAGE_BYTES};
use crate::error::CaptureError;
use crate::types::{CaptureRequest, Picture, Screen, ScreenFrame};

/// The id of the one screen.
pub const SCREEN_ID: &str = "replay";
/// The longest file the backend reads.
pub const MAX_FILE_BYTES: u64 = 64 * 1024 * 1024;
const FILE_MAGIC: &[u8; 4] = b"FOLR";
const FILE_VERSION: u8 = 1;
const FILE_HEADER_LEN: usize = 8;
/// A gap between two recorded frames is played at most this long.
const LONGEST_GAP: Duration = Duration::from_secs(1);

/// Writes frames as a replay file.
///
/// # Errors
///
/// [`CaptureError::Failed`] for a frame the codec refuses, and for no frames.
pub fn write(frames: &[ScreenFrame]) -> Result<Vec<u8>, CaptureError> {
    if frames.is_empty() {
        return Err(CaptureError::Failed(
            "a replay file needs at least one frame".to_owned(),
        ));
    }
    let mut out = Vec::new();
    out.extend_from_slice(FILE_MAGIC);
    out.extend_from_slice(&[FILE_VERSION, 0, 0, 0]);
    for frame in frames {
        let message = codec::encode(frame)?;
        // A frame is at most 388,836 bytes, so the length fits a u32.
        let length = u32::try_from(message.len()).unwrap_or(u32::MAX);
        out.extend_from_slice(&length.to_le_bytes());
        out.extend_from_slice(&message);
    }
    Ok(out)
}

/// Reads the frames of a replay file. Everything is checked as the codec
/// checks untrusted frames.
///
/// # Errors
///
/// [`CaptureError::Failed`] naming what is wrong: the header, a record that
/// is cut short or too long, a frame the codec refuses, or no frames.
pub fn parse(bytes: &[u8]) -> Result<Vec<ScreenFrame>, CaptureError> {
    let bad = |why: String| CaptureError::Failed(format!("replay file: {why}"));
    if bytes.len() < FILE_HEADER_LEN || &bytes[0..4] != FILE_MAGIC {
        return Err(bad("the header is not FOLR".to_owned()));
    }
    if bytes[4] != FILE_VERSION {
        return Err(bad(format!("version {} is not 1", bytes[4])));
    }
    let mut frames = Vec::new();
    let mut rest = &bytes[FILE_HEADER_LEN..];
    while !rest.is_empty() {
        let record = frames.len();
        let Some((length, after)) = rest.split_first_chunk::<4>() else {
            return Err(bad(format!("record {record} is cut short")));
        };
        let length = usize::try_from(u32::from_le_bytes(*length)).unwrap_or(usize::MAX);
        if !(HEADER_LEN..=MAX_MESSAGE_BYTES).contains(&length) || length > after.len() {
            return Err(bad(format!(
                "record {record} has a length of {length} bytes"
            )));
        }
        let frame =
            codec::decode(&after[..length]).map_err(|e| bad(format!("record {record}: {e}")))?;
        frames.push(frame);
        rest = &after[length..];
    }
    if frames.is_empty() {
        return Err(bad("it has no frames".to_owned()));
    }
    Ok(frames)
}

#[derive(Debug)]
struct Playing {
    next: usize,
    rate_gap: Duration,
    next_due: Instant,
}

#[derive(Debug)]
pub struct ReplayBackend {
    frames: Vec<ScreenFrame>,
    paced: bool,
    playing: Option<Playing>,
}

impl ReplayBackend {
    /// A backend that plays `frames` at their recorded times, again from the
    /// first after the last. Their sequence numbers and times are only used
    /// for the gaps between them; the pacer numbers what it sends anew. The
    /// recorded size is delivered as it is: the request's box isn't applied.
    ///
    /// # Errors
    ///
    /// [`CaptureError::Failed`] for no frames.
    pub fn new(frames: Vec<ScreenFrame>, paced: bool) -> Result<Self, CaptureError> {
        if frames.is_empty() {
            return Err(CaptureError::Failed(
                "a replay needs at least one frame".to_owned(),
            ));
        }
        Ok(Self {
            frames,
            paced,
            playing: None,
        })
    }

    /// Opens a replay file.
    ///
    /// # Errors
    ///
    /// [`CaptureError::Failed`] when the file can't be read, is longer than
    /// [`MAX_FILE_BYTES`] or [`parse`] refuses it.
    pub fn open(path: &Path) -> Result<Self, CaptureError> {
        let failed = |e: std::io::Error| {
            CaptureError::Failed(format!("replay file {}: {e}", path.display()))
        };
        let file = std::fs::File::open(path).map_err(failed)?;
        if !file.metadata().map_err(failed)?.is_file() {
            return Err(CaptureError::Failed(format!(
                "replay file {} is not a regular file",
                path.display()
            )));
        }
        // Bounded by the read itself: a file that grows, or a device, can't get past the cap.
        let mut bytes = Vec::new();
        file.take(MAX_FILE_BYTES + 1)
            .read_to_end(&mut bytes)
            .map_err(failed)?;
        if bytes.len() as u64 > MAX_FILE_BYTES {
            return Err(CaptureError::Failed(format!(
                "replay file {} is longer than 64 MiB",
                path.display()
            )));
        }
        Self::new(parse(&bytes)?, true)
    }

    /// How long to wait after frame `index` before the one after it.
    fn gap_after(&self, index: usize, fallback: Duration) -> Duration {
        let Some(next) = self.frames.get(index + 1) else {
            return fallback;
        };
        let micros = next
            .timestamp_us
            .saturating_sub(self.frames[index].timestamp_us);
        Duration::from_micros(micros).min(LONGEST_GAP)
    }
}

impl CaptureBackend for ReplayBackend {
    fn name(&self) -> &'static str {
        "replay"
    }

    fn list_screens(&mut self) -> Result<Vec<Screen>, CaptureError> {
        let first = &self.frames[0].picture;
        Ok(vec![Screen {
            id: SCREEN_ID.to_owned(),
            name: "Replay".to_owned(),
            width: u32::from(first.width),
            height: u32::from(first.height),
            primary: true,
        }])
    }

    fn start(&mut self, request: &CaptureRequest) -> Result<(), CaptureError> {
        request.validate()?;
        if request.screen != SCREEN_ID {
            return Err(CaptureError::UnknownScreen(request.screen.clone()));
        }
        let rate_gap = Duration::from_secs_f64(1.0 / request.rate_hz);
        // Asked again, the replay goes on where it was.
        let next = self.playing.as_ref().map_or(0, |p| p.next);
        self.playing = Some(Playing {
            next,
            rate_gap,
            next_due: Instant::now(),
        });
        Ok(())
    }

    fn next_picture(&mut self, timeout: Duration) -> Result<Option<Picture>, CaptureError> {
        let paced = self.paced;
        let playing = self.playing.as_ref().ok_or(CaptureError::NotStarted)?;
        let (index, rate_gap, due) = (playing.next, playing.rate_gap, playing.next_due);
        if paced {
            let wait = due.saturating_duration_since(Instant::now());
            if wait > timeout {
                thread::sleep(timeout);
                return Ok(None);
            }
            thread::sleep(wait);
        }
        let gap = self.gap_after(index, rate_gap);
        let picture = self.frames[index].picture.clone();
        if let Some(playing) = self.playing.as_mut() {
            playing.next = (index + 1) % self.frames.len();
            playing.next_due = Instant::now() + gap;
        }
        Ok(Some(picture))
    }

    fn stop(&mut self) {
        self.playing = None;
    }
}
