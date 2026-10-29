//! Reading a device: its three documents, and the PIN when `/json/cfg` wants one.

use folo_provider::ProviderError;
use serde_json::{Value, json};

use crate::api::DeviceApi;
use crate::device::Device;

pub const INFO_PATH: &str = "/json/info";
pub const STATE_PATH: &str = "/json/state";
pub const CFG_PATH: &str = "/json/cfg";

/// `/json/cfg`, unlocking the device with `pin` first when it asks for one. WLED takes the PIN as
/// a `pin` key in any JSON `POST`; once right, the device stays unlocked for everyone until it
/// restarts or is locked again. `None` when the device wants a PIN we don't have.
///
/// # Errors
/// Anything but a PIN refusal.
pub async fn read_cfg(
    api: &dyn DeviceApi,
    pin: Option<&str>,
) -> Result<Option<Value>, ProviderError> {
    match api.get(CFG_PATH).await {
        Ok(cfg) => return Ok(Some(cfg)),
        Err(ProviderError::Refused(_)) if pin.is_none() => return Ok(None),
        Err(ProviderError::Refused(_)) => {}
        Err(e) => return Err(e),
    }
    if let Some(pin) = pin {
        api.post(STATE_PATH, &json!({"pin": pin})).await?;
    }
    match api.get(CFG_PATH).await {
        Ok(cfg) => Ok(Some(cfg)),
        Err(ProviderError::Refused(_)) => Err(ProviderError::Refused(
            "the device refused the settings PIN".into(),
        )),
        Err(e) => Err(e),
    }
}

/// Reads the device: info, state and, when it can be had, cfg.
///
/// # Errors
/// [`ProviderError`] when info or state can't be read.
pub async fn read_device(api: &dyn DeviceApi, pin: Option<&str>) -> Result<Device, ProviderError> {
    let info = api.get(INFO_PATH).await?;
    let state = api.get(STATE_PATH).await?;
    let cfg = read_cfg(api, pin).await?;
    Ok(Device::from_json(&info, &state, cfg.as_ref()))
}
