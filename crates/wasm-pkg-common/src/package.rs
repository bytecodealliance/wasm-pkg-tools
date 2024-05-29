use serde::{Deserialize, Serialize};

use crate::{label::Label, Error};

/// A package reference, consisting of kebab-case namespace and name, e.g. `wasm-pkg:loader`.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(into = "String", try_from = "String")]
pub struct PackageRef {
    namespace: Label,
    name: Label,
}

impl PackageRef {
    /// Returns the namespace of the package.
    pub fn namespace(&self) -> &Label {
        &self.namespace
    }

    /// Returns the name of the package.
    pub fn name(&self) -> &Label {
        &self.name
    }
}

impl std::fmt::Display for PackageRef {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}:{}", self.namespace, self.name)
    }
}

impl From<PackageRef> for String {
    fn from(value: PackageRef) -> Self {
        value.to_string()
    }
}

impl TryFrom<String> for PackageRef {
    type Error = Error;

    fn try_from(mut value: String) -> Result<Self, Self::Error> {
        let Some(colon) = value.find(':') else {
            return Err(Error::InvalidPackageRef("missing expected ':'".into()));
        };
        let name = value.split_off(colon + 1);
        value.truncate(colon);
        Ok(Self {
            namespace: value.parse()?,
            name: name.parse()?,
        })
    }
}

impl std::str::FromStr for PackageRef {
    type Err = Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        s.to_string().try_into()
    }
}
