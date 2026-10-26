//! DXGI Desktop Duplication (ADR-0015 D3): `DuplicateOutput1` on a device made
//! on the adapter that drives the output, offered BGRA8 and FP16. The newest
//! desktop image is copied, given back at once, and downscaled on the GPU by
//! the [`Presenter`].
//!
//! Outputs that are rotated aren't handled here: the duplication delivers
//! the unrotated image, and Windows.Graphics.Capture, which comes next in
//! `auto`, delivers it as it is shown.

use std::time::{Duration, Instant};

use folo_capture_core::types::{CaptureRequest, Picture};
use windows::Win32::Graphics::Direct3D11::ID3D11Texture2D;
use windows::Win32::Graphics::Dxgi::Common::{
    DXGI_MODE_ROTATION_IDENTITY, DXGI_MODE_ROTATION_UNSPECIFIED,
};
use windows::Win32::Graphics::Dxgi::{
    DXGI_OUTDUPL_FRAME_INFO, IDXGIAdapter, IDXGIOutput5, IDXGIOutputDuplication, IDXGIResource,
};
use windows::core::Interface as _;

use crate::engine::Grabber;
use crate::fault::{DXGI_ERROR_WAIT_TIMEOUT, E_INVALIDARG, Fault};
use crate::gpu::{BGRA8, FP16, Gpu};
use crate::monitors::Monitor;
use crate::present::Presenter;

pub struct DdaGrabber {
    duplication: IDXGIOutputDuplication,
    presenter: Presenter,
}

/// A refusal to open: `E_INVALIDARG` is what a duplication on another adapter than the
/// output's (a hybrid-GPU laptop) answers with, and means this backend can't.
fn open_fault(error: windows::core::Error) -> Fault {
    if error.code().0 == E_INVALIDARG {
        Fault::Unsupported
    } else {
        Fault::from(error)
    }
}

impl DdaGrabber {
    /// Starts duplicating `monitor`.
    ///
    /// # Errors
    ///
    /// [`Fault::Unsupported`] when this backend can't capture the monitor, and the fault
    /// Windows' error stands for otherwise: the secure desktop, the process limit, a lost device.
    pub fn open(monitor: &Monitor, request: &CaptureRequest) -> Result<Self, Fault> {
        let dxgi = monitor.dxgi.as_ref().ok_or(Fault::Unsupported)?;
        let adapter = dxgi.adapter.cast::<IDXGIAdapter>()?;
        let gpu = Gpu::on_adapter(&adapter)?;
        let output = dxgi
            .output
            .cast::<IDXGIOutput5>()
            .map_err(|_| Fault::Unsupported)?;
        let duplication = Self::duplicate(&output, &gpu, monitor.hdr)?;
        if !Self::is_upright(&duplication) {
            return Err(Fault::Unsupported);
        }
        Ok(Self {
            duplication,
            presenter: Presenter::new(gpu, request, monitor.hdr, monitor.sdr_white_nits)?,
        })
    }

    /// An HDR desktop is offered FP16 first, so that Windows hands over scRGB instead of
    /// tone-mapping it itself.
    #[allow(unsafe_code)] // DuplicateOutput1
    fn duplicate(
        output: &IDXGIOutput5,
        gpu: &Gpu,
        hdr: bool,
    ) -> Result<IDXGIOutputDuplication, Fault> {
        let formats = if hdr { [FP16, BGRA8] } else { [BGRA8, FP16] };
        // SAFETY: the device is borrowed for the call and the formats slice is valid for it;
        // no flags are set.
        unsafe { output.DuplicateOutput1(&gpu.device, 0, &formats) }.map_err(open_fault)
    }

    #[allow(unsafe_code)] // GetDesc
    fn is_upright(duplication: &IDXGIOutputDuplication) -> bool {
        // SAFETY: GetDesc has no arguments and returns the description by value.
        let rotation = unsafe { duplication.GetDesc() }.Rotation;
        rotation == DXGI_MODE_ROTATION_IDENTITY || rotation == DXGI_MODE_ROTATION_UNSPECIFIED
    }

    /// Waits up to `wait` for a new desktop image and copies it. `true` when one came.
    #[allow(unsafe_code)] // AcquireNextFrame and ReleaseFrame
    fn acquire(&mut self, wait: Duration) -> Result<bool, Fault> {
        let milliseconds = u32::try_from(wait.as_micros().div_ceil(1000)).unwrap_or(u32::MAX);
        let mut info = DXGI_OUTDUPL_FRAME_INFO::default();
        let mut resource: Option<IDXGIResource> = None;
        // SAFETY: both pointers are to locals that live for the call.
        let acquired = unsafe {
            self.duplication
                .AcquireNextFrame(milliseconds, &raw mut info, &raw mut resource)
        };
        match acquired {
            Ok(()) => {}
            Err(error) if error.code().0 == DXGI_ERROR_WAIT_TIMEOUT => return Ok(false),
            Err(error) => return Err(Fault::from(error)),
        }
        // A frame that was acquired is released whatever happens to its copy.
        let copied = self.copy_frame(&info, resource);
        // SAFETY: a frame is held (AcquireNextFrame succeeded) and released once.
        let released = unsafe { self.duplication.ReleaseFrame() };
        let new_image = copied?;
        released?;
        Ok(new_image)
    }

    /// A frame with no present time only moved the pointer: nothing on the desktop changed.
    fn copy_frame(
        &mut self,
        info: &DXGI_OUTDUPL_FRAME_INFO,
        resource: Option<IDXGIResource>,
    ) -> Result<bool, Fault> {
        if info.LastPresentTime == 0 {
            return Ok(false);
        }
        let texture = resource
            .ok_or_else(|| Fault::Other("a frame without a desktop image".to_owned()))?
            .cast::<ID3D11Texture2D>()?;
        self.presenter.protected = info.ProtectedContentMaskedOut.as_bool();
        self.presenter.take(&texture)?;
        Ok(true)
    }
}

impl Grabber for DdaGrabber {
    fn grab(&mut self, timeout: Duration) -> Result<Option<Picture>, Fault> {
        let deadline = Instant::now() + timeout;
        loop {
            if self.presenter.ready_in() == Some(Duration::ZERO) {
                return self.presenter.emit().map(Some);
            }
            let remaining = deadline.saturating_duration_since(Instant::now());
            self.acquire(self.presenter.wait_within(remaining))?;
            if Instant::now() >= deadline && self.presenter.ready_in() != Some(Duration::ZERO) {
                self.presenter.check_alive()?;
                return Ok(None);
            }
        }
    }

    fn apply(&mut self, request: &CaptureRequest) {
        self.presenter.apply(request);
    }
}
