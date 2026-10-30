//! The screens (ADR-0037 D15): the monitors of the desktop, each with its
//! device path as id, its pixel size, whether it is HDR, and the SDR white
//! level Windows shows SDR content at.
//!
//! The monitor list is `EnumDisplayMonitors`' (every monitor of the desktop,
//! whatever drives it, so a remote session or a basic display driver still
//! lists them). DXGI adds what Desktop Duplication needs (the adapter and the
//! output) and the HDR state; `QueryDisplayConfig` adds the monitor's device
//! path and name and its SDR white level.

use std::ffi::c_void;
use std::sync::Once;

use folo_capture_core::types::{MAX_SCREEN_ID_CHARS, Screen};
use windows::Win32::Devices::Display::{
    DISPLAYCONFIG_DEVICE_INFO_GET_SDR_WHITE_LEVEL, DISPLAYCONFIG_DEVICE_INFO_GET_SOURCE_NAME,
    DISPLAYCONFIG_DEVICE_INFO_GET_TARGET_NAME, DISPLAYCONFIG_DEVICE_INFO_HEADER,
    DISPLAYCONFIG_MODE_INFO, DISPLAYCONFIG_PATH_INFO, DISPLAYCONFIG_SDR_WHITE_LEVEL,
    DISPLAYCONFIG_SOURCE_DEVICE_NAME, DISPLAYCONFIG_TARGET_DEVICE_NAME, DisplayConfigGetDeviceInfo,
    GetDisplayConfigBufferSizes, QDC_ONLY_ACTIVE_PATHS, QueryDisplayConfig,
};
use windows::Win32::Foundation::{ERROR_INSUFFICIENT_BUFFER, ERROR_SUCCESS, LPARAM, RECT};
use windows::Win32::Graphics::Dxgi::Common::DXGI_COLOR_SPACE_RGB_FULL_G2084_NONE_P2020;
use windows::Win32::Graphics::Dxgi::{
    CreateDXGIFactory1, IDXGIAdapter1, IDXGIFactory1, IDXGIOutput, IDXGIOutput6,
};
use windows::Win32::Graphics::Gdi::{
    EnumDisplayMonitors, GetMonitorInfoW, HDC, HMONITOR, MONITORINFO, MONITORINFOEXW,
};
use windows::Win32::UI::HiDpi::{
    DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, SetProcessDpiAwarenessContext,
};
use windows::core::{BOOL, Interface as _};

use crate::fault::Fault;
use crate::tonemap;

/// `MONITORINFO.dwFlags`' bit for the primary monitor.
const MONITORINFOF_PRIMARY: u32 = 1;

/// The adapter and output DXGI has for a monitor.
#[derive(Clone)]
pub struct DxgiOutput {
    pub adapter: IDXGIAdapter1,
    pub output: IDXGIOutput,
}

/// A monitor of the desktop, as of the moment it was listed.
#[derive(Clone)]
pub struct Monitor {
    pub screen: Screen,
    /// The `HMONITOR`, as the number it is.
    pub handle: isize,
    /// Where it is on the desktop, in pixels: left, top, width, height.
    pub bounds: (i32, i32, u32, u32),
    /// `None` when DXGI doesn't know the monitor (a remote session): Desktop Duplication can't capture it.
    pub dxgi: Option<DxgiOutput>,
    /// The output's colour space is HDR10 (`G2084`).
    pub hdr: bool,
    /// What `DISPLAYCONFIG_SDR_WHITE_LEVEL` says, in nits.
    pub sdr_white_nits: f32,
}

/// What `QueryDisplayConfig` knows of a monitor.
#[derive(Debug, Clone, Default)]
struct PathFacts {
    gdi_device: String,
    friendly_name: String,
    device_path: String,
    sdr_white_nits: Option<f32>,
}

/// Lists the monitors of the desktop, the primary one first, then by position.
///
/// # Errors
///
/// A [`Fault`] when Windows can't enumerate the monitors.
pub fn enumerate() -> Result<Vec<Monitor>, Fault> {
    make_per_monitor_dpi_aware();
    let facts = path_facts();
    let outputs = dxgi_outputs();
    let mut monitors: Vec<Monitor> = monitor_handles()?
        .into_iter()
        .filter_map(|handle| describe(handle, &facts, &outputs))
        .collect();
    monitors.sort_by_key(|monitor| {
        (
            !monitor.screen.primary,
            monitor.bounds.0,
            monitor.bounds.1,
            monitor.screen.id.clone(),
        )
    });
    Ok(monitors)
}

/// The monitor with this screen id.
///
/// # Errors
///
/// [`Fault::ScreenGone`] when none has it, and a [`Fault`] when Windows can't enumerate.
pub fn find(screen_id: &str) -> Result<Monitor, Fault> {
    enumerate()?
        .into_iter()
        .find(|monitor| monitor.screen.id == screen_id)
        .ok_or(Fault::ScreenGone)
}

/// Where the monitor with this handle is now (left, top, width, height), or `None` when it
/// is gone.
#[must_use]
pub fn bounds_of(handle: isize) -> Option<(i32, i32, u32, u32)> {
    let rect = monitor_info(handle)?.monitorInfo.rcMonitor;
    Some((
        rect.left,
        rect.top,
        u32::try_from(rect.right - rect.left).ok()?,
        u32::try_from(rect.bottom - rect.top).ok()?,
    ))
}

fn describe(
    handle: isize,
    facts: &[PathFacts],
    outputs: &[(isize, DxgiOutput)],
) -> Option<Monitor> {
    let info = monitor_info(handle)?;
    let device = wide_to_string(&info.szDevice);
    let rect = info.monitorInfo.rcMonitor;
    let path = facts.iter().find(|path| path.gdi_device == device);
    let dxgi = outputs
        .iter()
        .find(|(candidate, _)| *candidate == handle)
        .map(|(_, output)| output.clone());
    let id = path
        .map(|path| path.device_path.as_str())
        .filter(|path| !path.is_empty())
        .unwrap_or(&device);
    let name = path
        .map(|path| path.friendly_name.as_str())
        .filter(|name| !name.is_empty())
        .unwrap_or(&device);
    let (width, height) = (
        u32::try_from(rect.right - rect.left).ok()?,
        u32::try_from(rect.bottom - rect.top).ok()?,
    );
    Some(Monitor {
        screen: Screen {
            id: id.chars().take(MAX_SCREEN_ID_CHARS).collect(),
            name: name.chars().take(MAX_SCREEN_ID_CHARS).collect(),
            width,
            height,
            primary: info.monitorInfo.dwFlags & MONITORINFOF_PRIMARY != 0,
        },
        handle,
        bounds: (rect.left, rect.top, width, height),
        hdr: dxgi
            .as_ref()
            .is_some_and(|dxgi| output_is_hdr(&dxgi.output)),
        dxgi,
        sdr_white_nits: path
            .and_then(|path| path.sdr_white_nits)
            .unwrap_or(tonemap::DEFAULT_SDR_WHITE_NITS),
    })
}

/// A wide string up to its NUL.
fn wide_to_string(wide: &[u16]) -> String {
    let end = wide
        .iter()
        .position(|&unit| unit == 0)
        .unwrap_or(wide.len());
    String::from_utf16_lossy(&wide[..end])
}

/// Pixel sizes and positions are the real ones only for a process that is per-monitor
/// DPI aware. This process is the agent, so it says so, once. A process that already
/// has an awareness keeps it, and the call fails harmlessly.
#[allow(unsafe_code)] // one Win32 call without pointers
fn make_per_monitor_dpi_aware() {
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        // SAFETY: the call takes a constant and changes only this process's awareness.
        let _ =
            unsafe { SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2) };
    });
}

#[allow(unsafe_code)] // the callback of EnumDisplayMonitors
unsafe extern "system" fn collect_monitor(
    monitor: HMONITOR,
    _context: HDC,
    _rect: *mut RECT,
    data: LPARAM,
) -> BOOL {
    // SAFETY: `data` is the address of the `Vec` that `monitor_handles` passed, which
    // outlives the call, and EnumDisplayMonitors runs this on the calling thread only.
    let handles = unsafe { &mut *(data.0 as *mut Vec<isize>) };
    handles.push(monitor.0 as isize);
    BOOL(1)
}

#[allow(unsafe_code)] // EnumDisplayMonitors with a callback and a pointer to a local
fn monitor_handles() -> Result<Vec<isize>, Fault> {
    let mut handles: Vec<isize> = Vec::new();
    // SAFETY: the pointer is to `handles`, alive for the whole call; no clip region is
    // given, and the callback is `collect_monitor`, which only pushes to the vector.
    let listed = unsafe {
        EnumDisplayMonitors(
            None,
            None,
            Some(collect_monitor),
            LPARAM(&raw mut handles as isize),
        )
    };
    if listed.as_bool() {
        Ok(handles)
    } else {
        Err(Fault::Other("Windows can't list the monitors".to_owned()))
    }
}

#[allow(unsafe_code)] // GetMonitorInfoW with a pointer to a local
fn monitor_info(handle: isize) -> Option<MONITORINFOEXW> {
    let mut info = MONITORINFOEXW::default();
    info.monitorInfo.cbSize = u32::try_from(size_of::<MONITORINFOEXW>()).ok()?;
    // SAFETY: the pointer is to `info`, whose `cbSize` says it is the extended struct,
    // for the call. A handle of a monitor that went away just fails.
    let found = unsafe {
        GetMonitorInfoW(
            HMONITOR(handle as *mut c_void),
            (&raw mut info).cast::<MONITORINFO>(),
        )
    };
    found.as_bool().then_some(info)
}

/// The HDR state of an output: its colour space is HDR10 (the one Windows reports for
/// an HDR desktop). An output that can't say is taken as SDR.
#[allow(unsafe_code)] // GetDesc1
fn output_is_hdr(output: &IDXGIOutput) -> bool {
    let Ok(output6) = output.cast::<IDXGIOutput6>() else {
        return false;
    };
    // SAFETY: GetDesc1 has no arguments and returns the description by value.
    unsafe { output6.GetDesc1() }
        .is_ok_and(|desc| desc.ColorSpace == DXGI_COLOR_SPACE_RGB_FULL_G2084_NONE_P2020)
}

/// Every output of every adapter, by the `HMONITOR` it drives.
#[allow(unsafe_code)] // DXGI enumeration
fn dxgi_outputs() -> Vec<(isize, DxgiOutput)> {
    // SAFETY: CreateDXGIFactory1 takes no arguments but the interface asked for.
    let Ok(factory) = (unsafe { CreateDXGIFactory1::<IDXGIFactory1>() }) else {
        return Vec::new();
    };
    let mut found = Vec::new();
    for adapter_index in 0.. {
        // SAFETY: enumeration by index; the end of the list is an error, which ends the loop.
        let Ok(adapter) = (unsafe { factory.EnumAdapters1(adapter_index) }) else {
            break;
        };
        for output_index in 0.. {
            // SAFETY: as above.
            let Ok(output) = (unsafe { adapter.EnumOutputs(output_index) }) else {
                break;
            };
            // SAFETY: GetDesc has no arguments and returns the description by value.
            let Ok(desc) = (unsafe { output.GetDesc() }) else {
                continue;
            };
            if desc.AttachedToDesktop.as_bool() {
                found.push((
                    desc.Monitor.0 as isize,
                    DxgiOutput {
                        adapter: adapter.clone(),
                        output,
                    },
                ));
            }
        }
    }
    found
}

/// What the active display paths say of each monitor.
fn path_facts() -> Vec<PathFacts> {
    active_paths()
        .iter()
        .map(|path| {
            let target = &path.targetInfo;
            let (friendly_name, device_path) =
                target_names(target.adapterId, target.id).unwrap_or_default();
            PathFacts {
                gdi_device: source_name(path.sourceInfo.adapterId, path.sourceInfo.id)
                    .unwrap_or_default(),
                friendly_name,
                device_path,
                sdr_white_nits: sdr_white_level(target.adapterId, target.id),
            }
        })
        .collect()
}

#[allow(unsafe_code)] // QueryDisplayConfig into vectors the sizes were asked for
fn active_paths() -> Vec<DISPLAYCONFIG_PATH_INFO> {
    // The set of paths can change between asking for the sizes and asking for the paths.
    for _ in 0..3 {
        let (mut path_count, mut mode_count) = (0_u32, 0_u32);
        // SAFETY: the pointers are to locals for the call.
        let sized = unsafe {
            GetDisplayConfigBufferSizes(
                QDC_ONLY_ACTIVE_PATHS,
                &raw mut path_count,
                &raw mut mode_count,
            )
        };
        if sized != ERROR_SUCCESS {
            return Vec::new();
        }
        let mut paths = vec![DISPLAYCONFIG_PATH_INFO::default(); path_count as usize];
        let mut modes = vec![DISPLAYCONFIG_MODE_INFO::default(); mode_count as usize];
        // SAFETY: the arrays hold the counts that are passed, and they are written
        // only up to them.
        let queried = unsafe {
            QueryDisplayConfig(
                QDC_ONLY_ACTIVE_PATHS,
                &raw mut path_count,
                paths.as_mut_ptr(),
                &raw mut mode_count,
                modes.as_mut_ptr(),
                None,
            )
        };
        if queried == ERROR_SUCCESS {
            paths.truncate(path_count as usize);
            return paths;
        }
        if queried != ERROR_INSUFFICIENT_BUFFER {
            return Vec::new();
        }
    }
    Vec::new()
}

fn header(
    kind: windows::Win32::Devices::Display::DISPLAYCONFIG_DEVICE_INFO_TYPE,
    size: usize,
    adapter: windows::Win32::Foundation::LUID,
    id: u32,
) -> DISPLAYCONFIG_DEVICE_INFO_HEADER {
    DISPLAYCONFIG_DEVICE_INFO_HEADER {
        r#type: kind,
        size: u32::try_from(size).unwrap_or(0),
        adapterId: adapter,
        id,
    }
}

#[allow(unsafe_code)] // DisplayConfigGetDeviceInfo with a pointer to a local
fn source_name(adapter: windows::Win32::Foundation::LUID, id: u32) -> Option<String> {
    let mut request = DISPLAYCONFIG_SOURCE_DEVICE_NAME {
        header: header(
            DISPLAYCONFIG_DEVICE_INFO_GET_SOURCE_NAME,
            size_of::<DISPLAYCONFIG_SOURCE_DEVICE_NAME>(),
            adapter,
            id,
        ),
        ..DISPLAYCONFIG_SOURCE_DEVICE_NAME::default()
    };
    // SAFETY: the pointer is to `request`, whose header gives its type and size, for the call.
    let status = unsafe { DisplayConfigGetDeviceInfo(&raw mut request.header) };
    (status == 0).then(|| wide_to_string(&request.viewGdiDeviceName))
}

#[allow(unsafe_code)] // DisplayConfigGetDeviceInfo with a pointer to a local
fn target_names(adapter: windows::Win32::Foundation::LUID, id: u32) -> Option<(String, String)> {
    let mut request = DISPLAYCONFIG_TARGET_DEVICE_NAME {
        header: header(
            DISPLAYCONFIG_DEVICE_INFO_GET_TARGET_NAME,
            size_of::<DISPLAYCONFIG_TARGET_DEVICE_NAME>(),
            adapter,
            id,
        ),
        ..DISPLAYCONFIG_TARGET_DEVICE_NAME::default()
    };
    // SAFETY: the pointer is to `request`, whose header gives its type and size, for the call.
    let status = unsafe { DisplayConfigGetDeviceInfo(&raw mut request.header) };
    (status == 0).then(|| {
        (
            wide_to_string(&request.monitorFriendlyDeviceName),
            wide_to_string(&request.monitorDevicePath),
        )
    })
}

#[allow(unsafe_code)] // DisplayConfigGetDeviceInfo with a pointer to a local
fn sdr_white_level(adapter: windows::Win32::Foundation::LUID, id: u32) -> Option<f32> {
    let mut request = DISPLAYCONFIG_SDR_WHITE_LEVEL {
        header: header(
            DISPLAYCONFIG_DEVICE_INFO_GET_SDR_WHITE_LEVEL,
            size_of::<DISPLAYCONFIG_SDR_WHITE_LEVEL>(),
            adapter,
            id,
        ),
        ..DISPLAYCONFIG_SDR_WHITE_LEVEL::default()
    };
    // SAFETY: the pointer is to `request`, whose header gives its type and size, for the call.
    let status = unsafe { DisplayConfigGetDeviceInfo(&raw mut request.header) };
    (status == 0).then(|| tonemap::sdr_white_nits(request.SDRWhiteLevel))
}
