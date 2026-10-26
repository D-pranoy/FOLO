//! Windows.Graphics.Capture (ADR-0015 D3): for the outputs Desktop
//! Duplication can't capture (a hybrid-GPU laptop capturing against the
//! discrete GPU, rotated outputs, a remote session). The settings are the
//! ones of the ADR: the cursor off, a free-threaded frame pool (frames arrive
//! on a thread of the system's, with no dispatcher queue needed), and no
//! border where Windows allows it (build 20348 and newer, with its consent).
//!
//! Each frame is copied into the [`Presenter`]'s texture and given back to
//! the pool at once, so the pool's two buffers are never held.

use std::sync::{Arc, Condvar, Mutex, PoisonError, mpsc};
use std::thread;
use std::time::{Duration, Instant};

use folo_capture_core::types::{CaptureRequest, Picture};
use windows::Foundation::Metadata::ApiInformation;
use windows::Foundation::TypedEventHandler;
use windows::Graphics::Capture::{
    Direct3D11CaptureFrame, Direct3D11CaptureFramePool, GraphicsCaptureAccess,
    GraphicsCaptureAccessKind, GraphicsCaptureItem, GraphicsCaptureSession,
};
use windows::Graphics::DirectX::Direct3D11::IDirect3DDevice;
use windows::Graphics::DirectX::DirectXPixelFormat;
use windows::Graphics::SizeInt32;
use windows::Security::Authorization::AppCapabilityAccess::AppCapabilityAccessStatus;
use windows::Win32::Graphics::Direct3D11::ID3D11Texture2D;
use windows::Win32::Graphics::Dxgi::{IDXGIAdapter, IDXGIDevice};
use windows::Win32::Graphics::Gdi::HMONITOR;
use windows::Win32::System::WinRT::Direct3D11::{
    CreateDirect3D11DeviceFromDXGIDevice, IDirect3DDxgiInterfaceAccess,
};
use windows::Win32::System::WinRT::Graphics::Capture::IGraphicsCaptureItemInterop;
use windows::core::{IInspectable, Interface as _, h};

use crate::com;
use crate::engine::Grabber;
use crate::fault::{E_ACCESSDENIED, Fault};
use crate::gpu::Gpu;
use crate::monitors::Monitor;
use crate::present::Presenter;

/// Two buffers: one being written, one being copied.
const POOL_BUFFERS: i32 = 2;

/// What the pool's events leave for the grabbing thread.
#[derive(Default)]
struct Events {
    frame_waiting: bool,
    item_closed: bool,
}

#[derive(Default)]
struct Signal {
    events: Mutex<Events>,
    changed: Condvar,
}

impl Signal {
    fn raise(&self, change: impl FnOnce(&mut Events)) {
        let mut events = self.events.lock().unwrap_or_else(PoisonError::into_inner);
        change(&mut events);
        self.changed.notify_all();
    }

    /// Waits up to `wait` for something to happen, and says whether the item closed.
    fn wait(&self, wait: Duration) -> bool {
        let events = self.events.lock().unwrap_or_else(PoisonError::into_inner);
        let (mut events, _) = self
            .changed
            .wait_timeout_while(events, wait, |events| {
                !events.frame_waiting && !events.item_closed
            })
            .unwrap_or_else(PoisonError::into_inner);
        events.frame_waiting = false;
        events.item_closed
    }
}

pub struct WgcGrabber {
    gpu: Gpu,
    format: DirectXPixelFormat,
    pool: Direct3D11CaptureFramePool,
    pool_size: SizeInt32,
    session: GraphicsCaptureSession,
    item: GraphicsCaptureItem,
    /// The tokens of the two handlers, once they are added.
    tokens: Option<(i64, i64)>,
    signal: Arc<Signal>,
    presenter: Presenter,
}

/// A denial while opening is Windows' privacy setting for graphics capture, not a fault of the
/// capture, and no other backend may go around it.
fn open_fault(error: windows::core::Error) -> Fault {
    if error.code().0 == E_ACCESSDENIED {
        Fault::PrivacyDenied
    } else {
        Fault::from(error)
    }
}

/// How long the borderless consent is waited for. Past it the capture starts with the border.
const CONSENT_WAIT: Duration = Duration::from_secs(1);

impl WgcGrabber {
    /// Starts capturing `monitor`.
    ///
    /// Everything that can fail is made first, and the grabber (whose `Drop` closes what exists)
    /// is built before the handlers are added and the capture started, so a failure halfway
    /// leaves nothing open.
    ///
    /// # Errors
    ///
    /// [`Fault::Unsupported`] when this Windows has no graphics capture,
    /// [`Fault::PrivacyDenied`] when the privacy setting refuses it, and the fault a Windows
    /// error stands for otherwise.
    pub fn open(monitor: &Monitor, request: &CaptureRequest) -> Result<Self, Fault> {
        if !GraphicsCaptureSession::IsSupported().unwrap_or(false) {
            return Err(Fault::Unsupported);
        }
        let gpu = match &monitor.dxgi {
            Some(dxgi) => Gpu::on_adapter(&dxgi.adapter.cast::<IDXGIAdapter>()?)?,
            None => Gpu::hardware()?,
        };
        let presenter = Presenter::new(gpu.clone(), request, monitor.hdr, monitor.sdr_white_nits)?;
        let device = Self::winrt_device(&gpu)?;
        let item = Self::item_for(monitor)?;
        let format = if monitor.hdr {
            DirectXPixelFormat::R16G16B16A16Float
        } else {
            DirectXPixelFormat::B8G8R8A8UIntNormalized
        };
        let pool_size = item.Size()?;
        let pool = Direct3D11CaptureFramePool::CreateFreeThreaded(
            &device,
            format,
            POOL_BUFFERS,
            pool_size,
        )?;
        let session = pool.CreateCaptureSession(&item).map_err(open_fault)?;
        let mut grabber = Self {
            gpu,
            format,
            pool,
            pool_size,
            session,
            item,
            tokens: None,
            signal: Arc::new(Signal::default()),
            presenter,
        };
        grabber.start()?;
        Ok(grabber)
    }

    fn start(&mut self) -> Result<(), Fault> {
        self.tokens = Some(Self::subscribe(&self.pool, &self.item, &self.signal)?);
        Self::configure(&self.session);
        self.session.StartCapture().map_err(open_fault)
    }

    #[allow(unsafe_code)] // CreateDirect3D11DeviceFromDXGIDevice
    fn winrt_device(gpu: &Gpu) -> Result<IDirect3DDevice, Fault> {
        // The frame pool and its surfaces are used by threads of the system's.
        gpu.protect_for_threads();
        let dxgi_device = gpu.device.cast::<IDXGIDevice>()?;
        // SAFETY: the DXGI device is borrowed for the call, which wraps it in a WinRT object
        // that holds its own reference.
        let inspectable: IInspectable =
            unsafe { CreateDirect3D11DeviceFromDXGIDevice(&dxgi_device) }?;
        Ok(inspectable.cast::<IDirect3DDevice>()?)
    }

    #[allow(unsafe_code)] // CreateForMonitor
    fn item_for(monitor: &Monitor) -> Result<GraphicsCaptureItem, Fault> {
        let interop = windows::core::factory::<GraphicsCaptureItem, IGraphicsCaptureItemInterop>()?;
        // SAFETY: the handle is a monitor's, a plain number; a monitor that has gone away just
        // makes the call fail.
        unsafe {
            interop.CreateForMonitor::<GraphicsCaptureItem>(HMONITOR(monitor.handle as *mut _))
        }
        .map_err(open_fault)
    }

    fn subscribe(
        pool: &Direct3D11CaptureFramePool,
        item: &GraphicsCaptureItem,
        signal: &Arc<Signal>,
    ) -> Result<(i64, i64), Fault> {
        let on_frame = Arc::clone(signal);
        let arrived = pool.FrameArrived(&TypedEventHandler::<
            Direct3D11CaptureFramePool,
            IInspectable,
        >::new(move |_, _| {
            on_frame.raise(|events| events.frame_waiting = true);
            Ok(())
        }))?;
        let on_close = Arc::clone(signal);
        let closed = item.Closed(
            &TypedEventHandler::<GraphicsCaptureItem, IInspectable>::new(move |_, _| {
                on_close.raise(|events| events.item_closed = true);
                Ok(())
            }),
        )?;
        Ok((arrived, closed))
    }

    /// The cursor is never captured, and the border is turned off where Windows has the
    /// setting (build 20348 and newer) and the user consents to it through the access API.
    /// Without consent, or when the consent isn't given within [`CONSENT_WAIT`], the yellow
    /// border stays. Neither can fail the capture.
    fn configure(session: &GraphicsCaptureSession) {
        let _ = session.SetIsCursorCaptureEnabled(false);
        let has_border_setting = ApiInformation::IsPropertyPresent(
            h!("Windows.Graphics.Capture.GraphicsCaptureSession"),
            h!("IsBorderRequired"),
        )
        .unwrap_or(false);
        if has_border_setting && Self::borderless_allowed() {
            let _ = session.SetIsBorderRequired(false);
        }
    }

    /// Asks Windows for borderless access on a thread of its own, so that a consent dialog
    /// can't hold the capture thread past `next_picture`'s timeout.
    fn borderless_allowed() -> bool {
        let (sender, receiver) = mpsc::channel();
        thread::spawn(move || {
            com::ensure_initialized();
            let status =
                GraphicsCaptureAccess::RequestAccessAsync(GraphicsCaptureAccessKind::Borderless)
                    .and_then(|asking| asking.join());
            let _ = sender
                .send(status.is_ok_and(|status| status == AppCapabilityAccessStatus::Allowed));
        });
        receiver.recv_timeout(CONSENT_WAIT).unwrap_or(false)
    }

    /// Copies every frame that is waiting, newest last, and gives each back to the pool.
    fn drain(&mut self) -> Result<(), Fault> {
        while let Ok(frame) = self.pool.TryGetNextFrame() {
            let result = self.take_frame(&frame);
            let _ = frame.Close();
            result?;
        }
        Ok(())
    }

    #[allow(unsafe_code)] // GetInterface on the surface's DXGI access
    fn take_frame(&mut self, frame: &Direct3D11CaptureFrame) -> Result<(), Fault> {
        let size = frame.ContentSize()?;
        if (size.Width, size.Height) != (self.pool_size.Width, self.pool_size.Height) {
            // A mode change: the pool's buffers are the old size. This frame is dropped.
            let device = Self::winrt_device(&self.gpu)?;
            self.pool
                .Recreate(&device, self.format, POOL_BUFFERS, size)?;
            self.pool_size = size;
            return Ok(());
        }
        let access = frame.Surface()?.cast::<IDirect3DDxgiInterfaceAccess>()?;
        // SAFETY: the surface of a capture frame is a Direct3D 11 texture, and the interface
        // access hands out a reference to it that this call owns.
        let texture: ID3D11Texture2D = unsafe { access.GetInterface() }?;
        self.presenter.take(&texture)
    }
}

impl Drop for WgcGrabber {
    fn drop(&mut self) {
        if let Some((arrived, closed)) = self.tokens {
            let _ = self.pool.RemoveFrameArrived(arrived);
            let _ = self.item.RemoveClosed(closed);
        }
        let _ = self.session.Close();
        let _ = self.pool.Close();
    }
}

impl Grabber for WgcGrabber {
    fn grab(&mut self, timeout: Duration) -> Result<Option<Picture>, Fault> {
        let deadline = Instant::now() + timeout;
        loop {
            self.drain()?;
            if self.presenter.ready_in() == Some(Duration::ZERO) {
                return self.presenter.emit().map(Some);
            }
            let remaining = deadline.saturating_duration_since(Instant::now());
            if self.signal.wait(self.presenter.wait_within(remaining)) {
                return Err(Fault::AccessLost);
            }
            if Instant::now() >= deadline {
                self.drain()?;
                let ready = self.presenter.ready_in() == Some(Duration::ZERO);
                if ready {
                    return self.presenter.emit().map(Some);
                }
                // Frames come only when the screen changes, so none is not a fault, but a lost
                // GPU would look the same.
                self.presenter.check_alive()?;
                return Ok(None);
            }
        }
    }

    fn apply(&mut self, request: &CaptureRequest) {
        self.presenter.apply(request);
    }
}
