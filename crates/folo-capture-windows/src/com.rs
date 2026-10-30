//! COM on the calling thread. Windows.Graphics.Capture is `WinRT`, which needs
//! the thread's apartment; the backend can be moved to the thread that
//! captures, so every public entry point calls [`ensure_initialized`].

use std::cell::Cell;

use windows::Win32::System::Com::{COINIT_MULTITHREADED, CoInitializeEx};

thread_local! {
    static INITIALIZED: Cell<bool> = const { Cell::new(false) };
}

/// Joins the calling thread to the multithreaded apartment, once. A thread
/// that is already in a single-threaded apartment keeps it: DXGI, D3D11 and
/// free-threaded `WinRT` objects work in either.
#[allow(unsafe_code)] // one Win32 call, with no pointers
pub fn ensure_initialized() {
    INITIALIZED.with(|done| {
        if !done.get() {
            // SAFETY: CoInitializeEx takes no pointers; its result (S_OK, S_FALSE
            // or RPC_E_CHANGED_MODE) says which apartment the thread has, and all
            // three are fine here. The thread keeps it until it ends.
            let _ = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
            done.set(true);
        }
    });
}
