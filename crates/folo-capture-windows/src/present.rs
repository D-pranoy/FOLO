//! What the GPU grabbers share (Desktop Duplication and
//! Windows.Graphics.Capture): a copy of the newest desktop image that stays
//! ours after the frame is given back, the rate gate, and the downscale to a
//! [`Picture`].
//!
//! A desktop that changes faster than the rate asked for is copied every
//! time (one cheap GPU copy) and downscaled only when the gate allows, so the
//! picture sent is always the newest one, also when the screen then goes still.

use std::time::{Duration, Instant};

use folo_capture_core::sizing::fit_in_box;
use folo_capture_core::types::{Bars, CaptureRequest, FrameFlags, Picture};
use windows::Win32::Graphics::Direct3D11::{
    D3D11_TEXTURE2D_DESC, D3D11_USAGE_DEFAULT, ID3D11Texture2D,
};
use windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT;

use crate::fault::Fault;
use crate::gpu::{Downscaler, FP16, Gpu, shader_resource_binding};
use crate::throttle::RateGate;
use crate::tonemap;

/// The shape of a texture, to tell whether the copy still fits it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Shape {
    width: u32,
    height: u32,
    format: DXGI_FORMAT,
}

pub struct Presenter {
    gpu: Gpu,
    downscaler: Downscaler,
    copy: Option<(Shape, ID3D11Texture2D)>,
    /// A desktop image was copied and not yet turned into a picture.
    dirty: bool,
    gate: RateGate,
    started: Instant,
    box_size: (u16, u16),
    hdr: bool,
    hdr_scale: f32,
    /// Protected content is on screen (Desktop Duplication masked it out).
    pub protected: bool,
}

impl Presenter {
    /// A presenter on `gpu`. `hdr` is whether the desktop is HDR (the frames say so, and an
    /// FP16 image is tone-mapped against `sdr_white_nits`).
    ///
    /// # Errors
    ///
    /// A [`Fault`] when the shaders can't be made.
    pub fn new(
        gpu: Gpu,
        request: &CaptureRequest,
        hdr: bool,
        sdr_white_nits: f32,
    ) -> Result<Self, Fault> {
        Ok(Self {
            downscaler: Downscaler::new(gpu.clone())?,
            gpu,
            copy: None,
            dirty: false,
            gate: RateGate::new(request.rate_hz),
            started: Instant::now(),
            box_size: (request.width, request.height),
            hdr,
            hdr_scale: tonemap::scale_for(sdr_white_nits),
            protected: false,
        })
    }

    /// Applies the request's size and rate in place.
    pub fn apply(&mut self, request: &CaptureRequest) {
        self.gate.set_rate(request.rate_hz);
        self.box_size = (request.width, request.height);
    }

    fn now_us(&self) -> u64 {
        u64::try_from(self.started.elapsed().as_micros()).unwrap_or(u64::MAX)
    }

    /// Copies `image` (a desktop texture, which may be given back to its owner at once).
    ///
    /// # Errors
    ///
    /// A [`Fault`] when the copy can't be made.
    #[allow(unsafe_code)] // CopyResource
    pub fn take(&mut self, image: &ID3D11Texture2D) -> Result<(), Fault> {
        let desc = Gpu::describe(image);
        let shape = Shape {
            width: desc.Width,
            height: desc.Height,
            format: desc.Format,
        };
        if self.copy.as_ref().is_none_or(|(have, _)| *have != shape) {
            let made = D3D11_TEXTURE2D_DESC {
                MipLevels: 1,
                ArraySize: 1,
                Usage: D3D11_USAGE_DEFAULT,
                BindFlags: shader_resource_binding(),
                CPUAccessFlags: 0,
                MiscFlags: 0,
                ..desc
            };
            self.copy = Some((shape, self.gpu.texture(&made)?));
        }
        if let Some((_, copy)) = &self.copy {
            // SAFETY: both textures are alive, of the same size and format, and the copy is not
            // bound as a shader resource now (the downscaler unbinds it after each draw).
            unsafe { self.gpu.context.CopyResource(copy, image) };
            self.dirty = true;
        }
        Ok(())
    }

    /// How long until a picture can be made: `Some(ZERO)` now, `None` when no new image came.
    #[must_use]
    pub fn ready_in(&self) -> Option<Duration> {
        self.dirty.then(|| self.gate.wait(self.now_us()))
    }

    /// Downscales the newest copy to a picture that fits the requested box.
    ///
    /// # Errors
    ///
    /// A [`Fault`] when the GPU refuses; [`Fault::Other`] when no image was taken.
    pub fn emit(&mut self) -> Result<Picture, Fault> {
        // The rate is counted from the start of the work, not its end, or the work's own time
        // would be added to every period.
        let started = self.now_us();
        let Some((shape, copy)) = &self.copy else {
            return Err(Fault::Other("no desktop image was taken".to_owned()));
        };
        let size = fit_in_box(shape.width, shape.height, self.box_size.0, self.box_size.1);
        let tone_map = (shape.format == FP16).then_some(self.hdr_scale);
        let pixels = self.downscaler.downscale(copy, size, tone_map)?;
        self.dirty = false;
        self.gate.mark(started);
        Ok(Picture {
            screen_index: 0,
            width: size.0,
            height: size.1,
            flags: FrameFlags {
                protected: self.protected,
                hdr: self.hdr,
                held: false,
            },
            bars: Bars::default(),
            pixels,
        })
    }

    /// The most the caller should wait for the next image or for the gate, within `remaining`.
    #[must_use]
    pub fn wait_within(&self, remaining: Duration) -> Duration {
        self.ready_in()
            .map_or(remaining, |wait| wait.min(remaining))
    }
}

impl Presenter {
    /// Whether the device is still there. Called when a grab ended without a picture, because a
    /// lost GPU is otherwise indistinguishable from a screen that doesn't change.
    ///
    /// # Errors
    ///
    /// [`Fault::DeviceLost`] when the GPU was removed, reset or hung.
    pub fn check_alive(&self) -> Result<(), Fault> {
        self.gpu.check_alive()
    }
}
