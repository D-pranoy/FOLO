//! Pairing with a bridge through its link button (`docs/integrations/hue.md`, pairing).
//!
//! The person presses the button on the bridge; until then `POST /api` answers error 101. The
//! credentials that come back are secrets: they go to the node's secrets store
//! ([`Credentials::secrets`]) and nowhere else.

use std::fmt;
use std::time::Duration;

use folo_protocol::generated::{
    HuePairing, HuePairingMessage, HuePairingState, ProviderSecrets, ProviderSecretsAppKey,
    ProviderSecretsClientKey,
};
use folo_provider::ProviderError;
use serde_json::Value;

use crate::rest::{BridgeClient, RestError};

/// What a successful pairing yields.
#[derive(Clone)]
pub struct Credentials {
    /// The application key (`username` in the old API).
    pub app_key: String,
    /// The entertainment client key: 16 bytes as hex.
    pub client_key: String,
    /// The application id, which is the DTLS identity and not a secret.
    pub application_id: String,
}

impl fmt::Debug for Credentials {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Credentials")
            .field("application_id", &self.application_id)
            .finish_non_exhaustive()
    }
}

impl Credentials {
    /// The part to keep in `secrets.json`. The application id is read again at every connection.
    ///
    /// # Errors
    /// When a key is longer than the contract allows.
    pub fn secrets(&self) -> Result<ProviderSecrets, ProviderError> {
        let invalid = |e: String| ProviderError::Failed(format!("the bridge's credentials: {e}"));
        Ok(ProviderSecrets {
            app_key: Some(
                self.app_key
                    .parse::<ProviderSecretsAppKey>()
                    .map_err(|e| invalid(e.to_string()))?,
            ),
            client_key: Some(
                self.client_key
                    .parse::<ProviderSecretsClientKey>()
                    .map_err(|e| invalid(e.to_string()))?,
            ),
            ..ProviderSecrets::default()
        })
    }
}

/// How pairing goes.
#[derive(Clone, Debug)]
pub struct PairingSettings {
    /// The name of this computer, for the bridge's list of apps.
    pub device_name: String,
    /// How long to wait for the button.
    pub window: Duration,
    /// How often to ask the bridge.
    pub poll: Duration,
}

impl PairingSettings {
    /// The button is waited for 30 seconds, asking once a second.
    #[must_use]
    pub fn new(device_name: impl Into<String>) -> Self {
        Self {
            device_name: device_name.into(),
            window: Duration::from_secs(30),
            poll: Duration::from_secs(1),
        }
    }

    /// `folo#<device>`: at most 20 characters for the app and 19 for the device, letters,
    /// digits, `-` and `_` only.
    #[must_use]
    pub fn device_type(&self) -> String {
        let device: String = self
            .device_name
            .chars()
            .map(|c| {
                if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                    c
                } else {
                    '-'
                }
            })
            .take(19)
            .collect();
        format!("folo#{}", if device.is_empty() { "node" } else { &device })
    }
}

/// Where pairing is.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum PairingProgress {
    /// Waiting for the button, with this long left.
    Waiting {
        remaining: Duration,
    },
    Paired,
}

impl PairingProgress {
    /// The client API's `HuePairing`.
    ///
    /// # Panics
    /// Never: the messages are short constants.
    #[must_use]
    pub fn api(&self) -> HuePairing {
        let (state, message) = match self {
            Self::Waiting { remaining } => (
                HuePairingState::Waiting,
                format!(
                    "Press the link button on the bridge ({} s left)",
                    remaining.as_secs()
                ),
            ),
            Self::Paired => (HuePairingState::Paired, "Paired".to_owned()),
        };
        HuePairing {
            message: message.parse::<HuePairingMessage>().unwrap_or_else(|_| {
                "Press the link button on the bridge"
                    .parse()
                    .expect("a short message")
            }),
            state,
        }
    }
}

/// Pairing failed.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum PairingError {
    /// The button wasn't pressed in time.
    TimedOut,
    /// The bridge answered with an error other than "press the button".
    Bridge(String),
    Unreachable(String),
}

impl fmt::Display for PairingError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::TimedOut => f.write_str("the link button wasn't pressed in time"),
            Self::Bridge(m) => write!(f, "the bridge refused to pair: {m}"),
            Self::Unreachable(m) => f.write_str(m),
        }
    }
}

impl std::error::Error for PairingError {}

impl From<PairingError> for ProviderError {
    fn from(value: PairingError) -> Self {
        match value {
            PairingError::Unreachable(m) => Self::Unreachable(m),
            other => Self::Refused(other.to_string()),
        }
    }
}

/// One answer of `POST /api`.
enum Answer {
    ButtonNotPressed,
    Granted { app_key: String, client_key: String },
}

fn read_answer(answer: &Value) -> Result<Answer, PairingError> {
    let first = answer.as_array().and_then(|a| a.first()).unwrap_or(answer);
    if let Some(success) = first.get("success") {
        let text = |key: &str| success.get(key).and_then(Value::as_str).map(str::to_owned);
        return match (text("username"), text("clientkey")) {
            (Some(app_key), Some(client_key)) => Ok(Answer::Granted {
                app_key,
                client_key,
            }),
            _ => Err(PairingError::Bridge("the answer has no client key".into())),
        };
    }
    let error = first.get("error").unwrap_or(first);
    if error.get("type").and_then(Value::as_u64) == Some(101) {
        return Ok(Answer::ButtonNotPressed);
    }
    let description = error
        .get("description")
        .and_then(Value::as_str)
        .unwrap_or("unexpected answer");
    Err(PairingError::Bridge(description.to_owned()))
}

fn unreachable(e: &RestError) -> PairingError {
    PairingError::Unreachable(e.to_string())
}

/// Pairs: asks the bridge every `poll` until the button was pressed or the `window` is over,
/// telling `progress` before each wait, then reads the application id.
///
/// # Errors
/// [`PairingError`] when the button isn't pressed in time, the bridge refuses or can't be reached.
pub async fn pair(
    client: &BridgeClient,
    settings: &PairingSettings,
    progress: &mut dyn FnMut(PairingProgress),
) -> Result<Credentials, PairingError> {
    let device_type = settings.device_type();
    let started = tokio::time::Instant::now();
    let (app_key, client_key) = loop {
        match read_answer(
            &client
                .pair(&device_type)
                .await
                .map_err(|e| unreachable(&e))?,
        )? {
            Answer::Granted {
                app_key,
                client_key,
            } => break (app_key, client_key),
            Answer::ButtonNotPressed => {
                let elapsed = started.elapsed();
                if elapsed >= settings.window {
                    return Err(PairingError::TimedOut);
                }
                progress(PairingProgress::Waiting {
                    remaining: settings.window.saturating_sub(elapsed),
                });
                tokio::time::sleep(settings.poll).await;
            }
        }
    };
    let application_id = client
        .with_app_key(Some(app_key.clone()))
        .application_id()
        .await
        .map_err(|e| unreachable(&e))?;
    progress(PairingProgress::Paired);
    Ok(Credentials {
        app_key,
        client_key,
        application_id,
    })
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn the_device_type_fits_the_bridge_limits() {
        let settings = PairingSettings::new("Living room PC (main)!");
        let device_type = settings.device_type();
        assert_eq!(device_type, "folo#Living-room-PC--mai");
        assert!(device_type.len() <= "folo#".len() + 19);
        assert_eq!(PairingSettings::new("").device_type(), "folo#node");
    }

    #[test]
    fn answers_are_told_apart() {
        let pressed = json!([{"success": {"username": "u", "clientkey": "c"}}]);
        assert!(matches!(read_answer(&pressed), Ok(Answer::Granted { .. })));
        let waiting = json!([{"error": {"type": 101, "description": "link button not pressed"}}]);
        assert!(matches!(
            read_answer(&waiting),
            Ok(Answer::ButtonNotPressed)
        ));
        let other = json!([{"error": {"type": 7, "description": "invalid value"}}]);
        assert_eq!(
            read_answer(&other).err(),
            Some(PairingError::Bridge("invalid value".into()))
        );
        assert!(read_answer(&json!([{"success": {"username": "u"}}])).is_err());
    }
}
