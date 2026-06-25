use http::uri::Authority;
use serde::{Deserialize, Serialize};

use crate::Error;

/// A registry identifier.
///
/// This must be a valid HTTP Host.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(into = "String", try_from = "String")]
pub struct Registry(Authority);

impl Registry {
    /// Returns the registry host, without port number.
    pub fn host(&self) -> &str {
        self.0.host()
    }

    /// Returns the registry port number, if given.
    pub fn port(&self) -> Option<u16> {
        self.0.port_u16()
    }
}

impl AsRef<str> for Registry {
    fn as_ref(&self) -> &str {
        self.0.as_str()
    }
}

impl std::fmt::Display for Registry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl From<Registry> for String {
    fn from(value: Registry) -> Self {
        value.to_string()
    }
}

impl std::str::FromStr for Registry {
    type Err = Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(Self(s.parse()?))
    }
}

impl TryFrom<String> for Registry {
    type Error = Error;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Ok(Self(value.try_into()?))
    }
}
