//! GDI, the last resort (ADR-0015 D3): `BitBlt` of the monitor's rectangle
//! of the desktop into a DIB section, then the downscale on the CPU with
//! `folo-capture-core`'s reference (linear light, area weights). It works
//! where nothing else does and costs a full-resolution copy and a pass over
//! it, so it is the slowest, and it has no way to tell that nothing changed.
//!
//! The device context and the bitmap are kept between frames as the
//! numbers GDI hands out, so the grabber is plain data that can move between
//! threads (one at a time, which is how a capture is used).

use std::ffi::c_void;
use std::thread;
use std::time::{Duration, Instant};

use folo_capture_core::color;
use folo_capture_core::sizing::fit_in_box;
use folo_capture_core::types::{Bars, CaptureRequest, FrameFlags, Picture};
use windows::Win32::Foundation::HANDLE;
use windows::Win32::Graphics::Gdi::{
    BI_RGB, BITMAPINFO, BITMAPINFOHEADER, BitBlt, CAPTUREBLT, CreateCompatibleDC, CreateDIBSection,
    DIB_RGB_COLORS, DeleteDC, DeleteObject, GetDC, HBITMAP, HDC, HGDIOBJ, ReleaseDC, SRCCOPY,
    SelectObject,
};

use crate::engine::Grabber;
use crate::fault::Fault;
use crate::monitors::{self, Monitor};
use crate::throttle::RateGate;

/// The device context, the bitmap in it and what it held before, as GDI's handles and the
/// address of the bitmap's pixels.
struct Canvas {
    dc: isize,
    bitmap: isize,
    previous: isize,
    bits: usize,
}

pub struct GdiGrabber {
    handle: isize,
    bounds: (i32, i32, u32, u32),
    canvas: Canvas,
    gate: RateGate,
    started: Instant,
    box_size: (u16, u16),
    hdr: bool,
}

impl GdiGrabber {
    /// Prepares to capture `monitor`.
    ///
    /// # Errors
    ///
    /// [`Fault::Other`] when GDI can't make the bitmap (the monitor is larger than it can hold).
    pub fn open(monitor: &Monitor, request: &CaptureRequest) -> Result<Self, Fault> {
        Ok(Self {
            handle: monitor.handle,
            bounds: monitor.bounds,
            canvas: Self::make_canvas(monitor.bounds)?,
            gate: RateGate::new(request.rate_hz),
            started: Instant::now(),
            box_size: (request.width, request.height),
            hdr: monitor.hdr,
        })
    }

    #[allow(unsafe_code)] // creating a device context and a DIB section
    fn make_canvas(bounds: (i32, i32, u32, u32)) -> Result<Canvas, Fault> {
        let too_large = || Fault::Other("the monitor is too large for a GDI bitmap".to_owned());
        let (width, height) = (
            i32::try_from(bounds.2).map_err(|_| too_large())?,
            i32::try_from(bounds.3).map_err(|_| too_large())?,
        );
        let info = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: u32::try_from(size_of::<BITMAPINFOHEADER>()).map_err(|_| too_large())?,
                biWidth: width,
                // Negative: top-down, so the rows come in the order a picture has them.
                biHeight: -height,
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                ..BITMAPINFOHEADER::default()
            },
            ..BITMAPINFO::default()
        };
        let mut bits: *mut c_void = std::ptr::null_mut();
        // SAFETY: a memory device context compatible with the screen, and a DIB section whose
        // header describes a 32-bit top-down bitmap; `bits` is a local that lives for the call.
        // A failure frees what was made before it.
        unsafe {
            let dc = CreateCompatibleDC(None);
            if dc.is_invalid() {
                return Err(Fault::Other("GDI made no device context".to_owned()));
            }
            let bitmap = match CreateDIBSection(
                Some(dc),
                &raw const info,
                DIB_RGB_COLORS,
                &raw mut bits,
                None::<HANDLE>,
                0,
            ) {
                Ok(bitmap) if !bits.is_null() => bitmap,
                _ => {
                    let _ = DeleteDC(dc);
                    return Err(Fault::Other("GDI made no bitmap".to_owned()));
                }
            };
            let previous = SelectObject(dc, HGDIOBJ(bitmap.0));
            Ok(Canvas {
                dc: dc.0 as isize,
                bitmap: bitmap.0 as isize,
                previous: previous.0 as isize,
                bits: bits as usize,
            })
        }
    }

    /// Copies the monitor's rectangle of the desktop into the bitmap.
    #[allow(unsafe_code)] // GetDC, BitBlt, ReleaseDC
    fn copy_desktop(&self) -> Result<(), Fault> {
        // SAFETY: the screen's device context is got and released here, and the memory
        // context was made by `make_canvas` and is alive as long as `self`. BitBlt writes
        // `width × height` pixels into the bitmap, which has exactly that size.
        unsafe {
            let screen = GetDC(None);
            if screen.is_invalid() {
                // No device context for the screen: the desktop was switched away.
                return Err(Fault::SecureDesktop);
            }
            let copied = BitBlt(
                HDC(self.canvas.dc as *mut c_void),
                0,
                0,
                i32::try_from(self.bounds.2).unwrap_or(0),
                i32::try_from(self.bounds.3).unwrap_or(0),
                Some(screen),
                self.bounds.0,
                self.bounds.1,
                SRCCOPY | CAPTUREBLT,
            );
            let _ = ReleaseDC(None, screen);
            copied.map_err(Fault::from)
        }
    }

    #[allow(unsafe_code)] // reading the bitmap's pixels
    fn read_rgb(&self) -> Vec<u8> {
        let pixels = self.bounds.2 as usize * self.bounds.3 as usize;
        // SAFETY: `bits` is the address of the DIB section's `width × height` 32-bit pixels,
        // which stay alive and in place until the canvas is dropped; nothing else writes them
        // while this grabber reads them, because BitBlt is called on this thread only.
        let bgra = unsafe { std::slice::from_raw_parts(self.canvas.bits as *const u8, pixels * 4) };
        let mut rgb = Vec::with_capacity(pixels * 3);
        for pixel in bgra.as_chunks::<4>().0 {
            rgb.extend_from_slice(&[pixel[2], pixel[1], pixel[0]]);
        }
        rgb
    }

    fn picture(&self) -> Result<Picture, Fault> {
        let size = fit_in_box(
            self.bounds.2,
            self.bounds.3,
            self.box_size.0,
            self.box_size.1,
        );
        let pixels = color::downscale(
            &self.read_rgb(),
            self.bounds.2 as usize,
            self.bounds.3 as usize,
            usize::from(size.0),
            usize::from(size.1),
        )
        .map_err(|error| Fault::Other(error.to_string()))?;
        Ok(Picture {
            screen_index: 0,
            width: size.0,
            height: size.1,
            flags: FrameFlags {
                hdr: self.hdr,
                ..FrameFlags::default()
            },
            bars: Bars::default(),
            pixels,
        })
    }

    fn now_us(&self) -> u64 {
        u64::try_from(self.started.elapsed().as_micros()).unwrap_or(u64::MAX)
    }
}

impl Drop for Canvas {
    #[allow(unsafe_code)] // releasing the GDI objects made by `make_canvas`
    fn drop(&mut self) {
        // SAFETY: the handles are the ones `make_canvas` made, released once, in the order GDI
        // wants: the old object back into the context, then the bitmap, then the context.
        unsafe {
            let dc = HDC(self.dc as *mut c_void);
            let _ = SelectObject(dc, HGDIOBJ(self.previous as *mut c_void));
            let _ = DeleteObject(HGDIOBJ(HBITMAP(self.bitmap as *mut c_void).0));
            let _ = DeleteDC(dc);
        }
    }
}

impl Grabber for GdiGrabber {
    fn grab(&mut self, timeout: Duration) -> Result<Option<Picture>, Fault> {
        let wait = self.gate.wait(self.now_us());
        if wait > timeout {
            thread::sleep(timeout);
            return Ok(None);
        }
        thread::sleep(wait);
        // A mode change moves or resizes the monitor: the bitmap is the old size.
        if monitors::bounds_of(self.handle) != Some(self.bounds) {
            return Err(Fault::AccessLost);
        }
        self.copy_desktop()?;
        let picture = self.picture()?;
        self.gate.mark(self.now_us());
        Ok(Some(picture))
    }

    fn apply(&mut self, request: &CaptureRequest) {
        self.gate.set_rate(request.rate_hz);
        self.box_size = (request.width, request.height);
    }
}
