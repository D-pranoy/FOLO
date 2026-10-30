//! The scaling thread: turns the newest copied buffer into the picture the
//! reader takes (ADR-0037 D17), at most as often as the node asked for.

use std::sync::Arc;
use std::time::{Duration, Instant};

use folo_capture_core::error::CaptureError;
use folo_capture_core::sizing::fit_in_box;
use folo_capture_core::types::{Bars, FrameFlags, Picture};
use tokio::sync::mpsc::UnboundedSender;

use crate::handoff::Handoff;
use crate::scale::{Downscaler, SourceImage};
use crate::shared::Shared;
use crate::stream::StreamEvent;

/// The one screen's position in the agent's listing.
const SCREEN_INDEX: u16 = 0;

/// Turns the buffers of a stream into pictures for the reader.
pub struct FrameSink {
    shared: Arc<Shared>,
    downscaler: Option<Downscaler>,
}

impl FrameSink {
    pub fn new(shared: Arc<Shared>) -> Self {
        Self {
            shared,
            downscaler: None,
        }
    }

    /// Downscales one buffer to the size that fits the box the node asked
    /// for and publishes it.
    ///
    /// # Errors
    ///
    /// [`CaptureError::Failed`] for a buffer that doesn't match its size.
    pub fn deliver(&mut self, image: &SourceImage<'_>) -> Result<(), CaptureError> {
        let target = self.shared.target();
        let (width, height) = fit_in_box(
            u32::try_from(image.width).unwrap_or(u32::MAX),
            u32::try_from(image.height).unwrap_or(u32::MAX),
            target.box_width,
            target.box_height,
        );
        let output = (usize::from(width), usize::from(height));
        let input = (image.width, image.height);
        let downscaler = match self.downscaler.take() {
            Some(downscaler) if downscaler.is_for(input, output) => downscaler,
            _ => Downscaler::new(input.0, input.1, output.0, output.1)?,
        };
        let downscaler = self.downscaler.insert(downscaler);
        let pixels = downscaler.scale(image)?;
        self.shared.publish(Picture {
            screen_index: SCREEN_INDEX,
            width,
            height,
            flags: FrameFlags::default(),
            bars: Bars::default(),
            pixels,
        });
        Ok(())
    }

    fn interval(&self) -> Duration {
        Duration::from_secs_f64(1.0 / self.shared.target().rate_hz.clamp(1.0, 60.0))
    }
}

/// The scaling thread's body: scales the newest frame, no more often than
/// the rate asks, until the hand-over is closed.
pub fn run(handoff: &Handoff, mut sink: FrameSink, events: &UnboundedSender<StreamEvent>) {
    let mut not_before = Instant::now();
    while let Some(frame) = handoff.take_not_before(not_before) {
        let started = Instant::now();
        let image = SourceImage {
            layout: frame.layout,
            width: frame.width,
            height: frame.height,
            stride: frame.stride,
            data: &frame.bytes,
        };
        if let Err(error) = sink.deliver(&image) {
            let _ = events.send(StreamEvent::Ended(error.to_string()));
            return;
        }
        handoff.recycle(frame);
        not_before = started + sink.interval();
    }
}
