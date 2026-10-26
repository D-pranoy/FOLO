//! The display capture agent's Windows backend (RB-14, ADR-0015 D3,
//! ADR-0037 D18 to D20). It captures one monitor with DXGI Desktop
//! Duplication, Windows.Graphics.Capture or GDI, downscales on the GPU before
//! readback, tone-maps an HDR desktop, and recovers from the failures
//! Windows has for capturing.
//!
//! The modules that don't call Windows are built on every platform, so the
//! state machine and its policy are tested on Linux too:
//! - [`fault`]: what can go wrong, and the classification of `HRESULT`s
//! - [`recovery`]: the state machine: which fault retries, switches or gives up
//! - [`engine`]: [`engine::Engine`], the [`folo_capture_core::backend::CaptureBackend`] built on it
//! - [`throttle`] and [`tonemap`]: the rate gate, and the HDR reference
//!
//! The rest is Windows only, with `unsafe` in item-level `#[allow]`s with a
//! `SAFETY` comment (ADR-0037 D19):
//! - `clock`: the `QueryPerformanceCounter` clock that stamps frames (D6)
//! - `monitors`: the screens, their ids, HDR state and SDR white level
//! - `shader` and `gpu`: the HLSL downscaler, compiled at run time (D18)
//! - `dda`, `wgc` and `gdi`: the three grabbers, and `factory` that opens them

pub mod engine;
pub mod fault;
pub mod recovery;
pub mod throttle;
pub mod tonemap;

#[cfg(windows)]
pub mod clock;
#[cfg(windows)]
pub mod com;
#[cfg(windows)]
pub mod dda;
#[cfg(windows)]
pub mod factory;
#[cfg(windows)]
pub mod gdi;
#[cfg(windows)]
pub mod gpu;
#[cfg(windows)]
pub mod monitors;
#[cfg(windows)]
pub mod present;
#[cfg(windows)]
pub mod shader;
#[cfg(windows)]
pub mod wgc;
