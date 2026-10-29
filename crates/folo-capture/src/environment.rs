//! What the node's launch puts in the agent's environment (ADR-0037 D2):
//! the capture link's URL and the launch token. Never on the command line.

use std::fmt;

/// The capture link's URL.
pub const LINK_VARIABLE: &str = "FOLO_CAPTURE_LINK";
/// The 256-bit launch token, base64url.
pub const TOKEN_VARIABLE: &str = "FOLO_CAPTURE_TOKEN";

#[derive(Clone, PartialEq, Eq)]
pub struct Environment {
    pub link: String,
    pub token: String,
}

/// The variables that are missing or empty, by name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MissingVariables(pub Vec<&'static str>);

impl fmt::Display for MissingVariables {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "the node starts the agent with {} in its environment",
            self.0.join(" and ")
        )
    }
}

impl std::error::Error for MissingVariables {}

// The token stays out of any log line or panic message.
impl fmt::Debug for Environment {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Environment")
            .field("link", &self.link)
            .field("token", &"<hidden>")
            .finish()
    }
}

impl Environment {
    /// Reads the variables through `lookup` (the process environment in `run`).
    ///
    /// # Errors
    ///
    /// [`MissingVariables`] naming every variable that is unset or empty.
    pub fn read(lookup: impl Fn(&str) -> Option<String>) -> Result<Self, MissingVariables> {
        let value = |name: &str| lookup(name).filter(|value| !value.is_empty());
        match (value(LINK_VARIABLE), value(TOKEN_VARIABLE)) {
            (Some(link), Some(token)) => Ok(Self { link, token }),
            (link, token) => {
                let mut missing = Vec::new();
                if link.is_none() {
                    missing.push(LINK_VARIABLE);
                }
                if token.is_none() {
                    missing.push(TOKEN_VARIABLE);
                }
                Err(MissingVariables(missing))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lookup(link: Option<&str>, token: Option<&str>) -> impl Fn(&str) -> Option<String> {
        let (link, token) = (link.map(str::to_owned), token.map(str::to_owned));
        move |name| {
            if name == LINK_VARIABLE {
                link.clone()
            } else if name == TOKEN_VARIABLE {
                token.clone()
            } else {
                None
            }
        }
    }

    #[test]
    fn both_variables_make_an_environment_and_its_debug_hides_the_token() {
        let environment = Environment::read(lookup(Some("ws://x"), Some("secret"))).unwrap();
        assert_eq!(
            (environment.link.as_str(), environment.token.as_str()),
            ("ws://x", "secret")
        );
        let shown = format!("{environment:?}");
        assert!(
            shown.contains("ws://x") && !shown.contains("secret"),
            "{shown}"
        );
    }

    #[test]
    fn unset_and_empty_variables_are_named() {
        assert_eq!(
            Environment::read(lookup(None, None)).unwrap_err().0,
            [LINK_VARIABLE, TOKEN_VARIABLE]
        );
        assert_eq!(
            Environment::read(lookup(Some(""), Some("t")))
                .unwrap_err()
                .0,
            [LINK_VARIABLE]
        );
        assert_eq!(
            Environment::read(lookup(Some("l"), Some("")))
                .unwrap_err()
                .0,
            [TOKEN_VARIABLE]
        );
    }
}
