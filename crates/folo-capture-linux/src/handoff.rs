//! The hand-over between the PipeWire callbacks and the scaling thread.
//!
//! libpipewire may run a stream's `process` callback on its own data thread
//! as well as on the loop's thread, so the callback does the least it can: it
//! copies the buffer into a slot and returns the buffer to the compositor at
//! once, instead of holding it through the downscale. The scaling thread takes
//! the newest copy. A copy that nobody took is replaced by a newer one, but
//! the newest is never dropped, so the last picture before a still screen is
//! always scaled.

use std::sync::{Condvar, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

use crate::scale::PixelLayout;

/// One buffer's pixels as the compositor laid them out.
pub struct RawFrame {
    pub layout: PixelLayout,
    pub width: usize,
    pub height: usize,
    pub stride: usize,
    /// From the first row to the last pixel of the last row.
    pub bytes: Vec<u8>,
}

#[derive(Default)]
struct Slot {
    newest: Option<RawFrame>,
    /// A buffer to fill next, so frames of one size don't allocate.
    spare: Vec<u8>,
    closed: bool,
}

#[derive(Default)]
pub struct Handoff {
    slot: Mutex<Slot>,
    arrived: Condvar,
}

impl Handoff {
    fn lock(&self) -> MutexGuard<'_, Slot> {
        self.slot.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Copies `source` into the newest slot, replacing the one nobody took.
    pub fn put(
        &self,
        layout: PixelLayout,
        (width, height): (usize, usize),
        stride: usize,
        source: &[u8],
    ) {
        let mut slot = self.lock();
        let mut bytes = std::mem::take(&mut slot.spare);
        if let Some(replaced) = slot.newest.take() {
            // The older copy is the better spare: it has the capacity already.
            bytes = replaced.bytes;
        }
        bytes.clear();
        bytes.extend_from_slice(source);
        slot.newest = Some(RawFrame {
            layout,
            width,
            height,
            stride,
            bytes,
        });
        drop(slot);
        self.arrived.notify_one();
    }

    /// Waits for a frame, not before `not_before`. `None` once closed.
    pub fn take_not_before(&self, not_before: Instant) -> Option<RawFrame> {
        let mut slot = self.lock();
        loop {
            if slot.closed {
                return None;
            }
            let now = Instant::now();
            if slot.newest.is_some() && now >= not_before {
                return slot.newest.take();
            }
            let wait = if slot.newest.is_some() {
                not_before - now
            } else {
                Duration::from_secs(1)
            };
            slot = self
                .arrived
                .wait_timeout(slot, wait)
                .unwrap_or_else(PoisonError::into_inner)
                .0;
        }
    }

    /// Gives a scaled frame's memory back for the next copy.
    pub fn recycle(&self, frame: RawFrame) {
        let mut slot = self.lock();
        if slot.spare.capacity() < frame.bytes.capacity() {
            slot.spare = frame.bytes;
        }
    }

    pub fn close(&self) {
        self.lock().closed = true;
        self.arrived.notify_all();
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::thread;

    use super::*;

    fn put_marked(handoff: &Handoff, mark: u8) {
        handoff.put(PixelLayout::Bgrx, (1, 1), 4, &[mark; 4]);
    }

    #[test]
    fn the_newest_frame_replaces_one_nobody_took() {
        let handoff = Handoff::default();
        put_marked(&handoff, 1);
        put_marked(&handoff, 2);
        let frame = handoff.take_not_before(Instant::now()).unwrap();
        assert_eq!(frame.bytes, [2; 4]);
        handoff.close();
        assert!(handoff.take_not_before(Instant::now()).is_none());
    }

    #[test]
    fn a_frame_is_held_until_its_time_and_never_dropped() {
        let handoff = Handoff::default();
        put_marked(&handoff, 9);
        let started = Instant::now();
        let frame = handoff
            .take_not_before(started + Duration::from_millis(120))
            .unwrap();
        assert_eq!(frame.bytes, [9; 4]);
        assert!(started.elapsed() >= Duration::from_millis(100));
    }

    #[test]
    fn a_waiting_taker_wakes_for_a_frame_and_for_the_close() {
        let handoff = Arc::new(Handoff::default());
        let taker = {
            let handoff = Arc::clone(&handoff);
            thread::spawn(move || {
                let first = handoff.take_not_before(Instant::now()).map(|f| f.bytes);
                let second = handoff.take_not_before(Instant::now()).map(|f| f.bytes);
                (first, second)
            })
        };
        thread::sleep(Duration::from_millis(50));
        put_marked(&handoff, 7);
        thread::sleep(Duration::from_millis(50));
        handoff.close();
        let (first, second) = taker.join().unwrap();
        assert_eq!(first, Some(vec![7; 4]));
        assert_eq!(second, None);
    }

    #[test]
    fn memory_comes_back_for_the_next_copy() {
        let handoff = Handoff::default();
        put_marked(&handoff, 1);
        let frame = handoff.take_not_before(Instant::now()).unwrap();
        let capacity = frame.bytes.capacity();
        handoff.recycle(frame);
        put_marked(&handoff, 2);
        let next = handoff.take_not_before(Instant::now()).unwrap();
        assert_eq!(next.bytes, [2; 4]);
        assert_eq!(next.bytes.capacity(), capacity);
    }
}
