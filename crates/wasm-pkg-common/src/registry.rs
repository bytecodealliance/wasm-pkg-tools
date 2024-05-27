use http::uri::Authority;

use crate::Error;

/// A registry identifier, which should be a valid HTTP Host.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Registry(Authority);

impl Registry {
    pub fn host(&self) -> &str {
        self.0.host()
    }

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
