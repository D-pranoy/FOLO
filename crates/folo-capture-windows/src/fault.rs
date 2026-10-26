//! What can go wrong while capturing on Windows, as the recovery state
//! machine sees it (ADR-0015 D3, [display capture](../../../docs/integrations/display-capture.md#windows)).
//! The classification of `HRESULT`s is plain data, so it is tested on every platform.

use std::fmt;

/// A failure of a grabber or of opening one. The recovery state machine
/// ([`crate::recovery::Recovery`]) decides what each one means.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Fault {
    /// `DXGI_ERROR_ACCESS_LOST`: a mode change, the secure desktop (UAC, the lock screen), or a
    /// full-screen application took the output. The duplication must be made again.
    AccessLost,
    /// Desktop Duplication was denied (`E_ACCESSDENIED`): the secure desktop is up.
    SecureDesktop,
    /// `DXGI_ERROR_SESSION_DISCONNECTED`: the session is disconnected, locked remotely or switched away.
    SessionDisconnected,
    /// `DXGI_ERROR_UNSUPPORTED`: this backend can't capture this output (a hybrid-GPU laptop, a
    /// remote session, an old driver). Another backend might.
    Unsupported,
    /// `DXGI_ERROR_NOT_CURRENTLY_AVAILABLE`: the session already has four duplications.
    ProcessLimit,
    /// The GPU was removed, reset or hung: the device and everything made from it are gone.
    DeviceLost,
    /// Windows' "Screenshots and screen recording" setting (or its policy) denies graphics capture.
    /// Never worked around by another backend: the setting is the user's.
    PrivacyDenied,
    /// The screen isn't connected any more (an unplugged monitor).
    ScreenGone,
    /// Anything else, with the operating system's words for it.
    Other(String),
}

/// `HRESULT`s, written as the unsigned values the headers print.
const fn hresult(unsigned: u32) -> i32 {
    i32::from_ne_bytes(unsigned.to_ne_bytes())
}

pub const DXGI_ERROR_UNSUPPORTED: i32 = hresult(0x887A_0004);
pub const DXGI_ERROR_DEVICE_REMOVED: i32 = hresult(0x887A_0005);
pub const DXGI_ERROR_DEVICE_HUNG: i32 = hresult(0x887A_0006);
pub const DXGI_ERROR_DEVICE_RESET: i32 = hresult(0x887A_0007);
pub const DXGI_ERROR_NOT_CURRENTLY_AVAILABLE: i32 = hresult(0x887A_0022);
pub const DXGI_ERROR_ACCESS_LOST: i32 = hresult(0x887A_0026);
pub const DXGI_ERROR_WAIT_TIMEOUT: i32 = hresult(0x887A_0027);
pub const DXGI_ERROR_SESSION_DISCONNECTED: i32 = hresult(0x887A_0028);
pub const E_ACCESSDENIED: i32 = hresult(0x8007_0005);
pub const E_INVALIDARG: i32 = hresult(0x8007_0057);

impl Fault {
    /// The fault an `HRESULT` stands for. `E_ACCESSDENIED` is the secure desktop here: the
    /// Windows.Graphics.Capture grabber reports its own denial as [`Fault::PrivacyDenied`].
    #[must_use]
    pub fn from_hresult(code: i32, message: &str) -> Self {
        match code {
            DXGI_ERROR_ACCESS_LOST => Self::AccessLost,
            E_ACCESSDENIED => Self::SecureDesktop,
            DXGI_ERROR_SESSION_DISCONNECTED => Self::SessionDisconnected,
            DXGI_ERROR_UNSUPPORTED => Self::Unsupported,
            DXGI_ERROR_NOT_CURRENTLY_AVAILABLE => Self::ProcessLimit,
            DXGI_ERROR_DEVICE_REMOVED | DXGI_ERROR_DEVICE_HUNG | DXGI_ERROR_DEVICE_RESET => {
                Self::DeviceLost
            }
            _ => Self::Other(format!("{message} ({code:#010x})")),
        }
    }

    /// Whether the agent can't capture while this lasts: `paused`, with its last picture held.
    #[must_use]
    pub fn pauses(&self) -> bool {
        !matches!(self, Self::Other(_) | Self::Unsupported)
    }
}

impl fmt::Display for Fault {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::AccessLost => f.write_str("the screen was taken over (a mode change or the secure desktop)"),
            Self::SecureDesktop => f.write_str("Windows is showing the secure desktop (UAC or the lock screen)"),
            Self::SessionDisconnected => f.write_str("the Windows session is disconnected"),
            Self::Unsupported => f.write_str("this way of capturing doesn't work for this screen"),
            Self::ProcessLimit => f.write_str("too many programs are duplicating screens in this session"),
            Self::DeviceLost => f.write_str("the graphics device was lost"),
            Self::PrivacyDenied => f.write_str(
                "Windows' screenshots and screen recording setting doesn't let FOLO capture the screen",
            ),
            Self::ScreenGone => f.write_str("the screen isn't connected"),
            Self::Other(why) => f.write_str(why),
        }
    }
}

impl std::error::Error for Fault {}

#[cfg(windows)]
impl From<windows::core::Error> for Fault {
    fn from(error: windows::core::Error) -> Self {
        Self::from_hresult(error.code().0, &error.message())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_hresults_of_the_headers_have_the_values_microsoft_documents() {
        assert_eq!(DXGI_ERROR_ACCESS_LOST, -2_005_270_490);
        assert_eq!(DXGI_ERROR_UNSUPPORTED, -2_005_270_524);
        assert_eq!(DXGI_ERROR_SESSION_DISCONNECTED, -2_005_270_488);
        assert_eq!(DXGI_ERROR_NOT_CURRENTLY_AVAILABLE, -2_005_270_494);
        assert_eq!(E_ACCESSDENIED, -2_147_024_891);
    }

    #[test]
    fn each_documented_error_maps_to_its_fault() {
        for (code, fault) in [
            (DXGI_ERROR_ACCESS_LOST, Fault::AccessLost),
            (E_ACCESSDENIED, Fault::SecureDesktop),
            (DXGI_ERROR_SESSION_DISCONNECTED, Fault::SessionDisconnected),
            (DXGI_ERROR_UNSUPPORTED, Fault::Unsupported),
            (DXGI_ERROR_NOT_CURRENTLY_AVAILABLE, Fault::ProcessLimit),
            (DXGI_ERROR_DEVICE_REMOVED, Fault::DeviceLost),
            (DXGI_ERROR_DEVICE_HUNG, Fault::DeviceLost),
            (DXGI_ERROR_DEVICE_RESET, Fault::DeviceLost),
        ] {
            assert_eq!(Fault::from_hresult(code, "x"), fault, "{code:#x}");
        }
    }

    #[test]
    fn other_errors_keep_the_systems_words_and_the_code() {
        let fault = Fault::from_hresult(E_INVALIDARG, "The parameter is incorrect.");
        let text = fault.to_string();
        assert!(matches!(fault, Fault::Other(_)));
        assert!(
            text.contains("The parameter is incorrect.") && text.contains("0x80070057"),
            "{text}"
        );
    }

    #[test]
    fn only_a_screen_that_can_be_captured_later_pauses() {
        assert!(Fault::AccessLost.pauses() && Fault::SecureDesktop.pauses());
        assert!(Fault::PrivacyDenied.pauses() && Fault::ScreenGone.pauses());
        assert!(!Fault::Unsupported.pauses() && !Fault::Other(String::new()).pauses());
    }
}
