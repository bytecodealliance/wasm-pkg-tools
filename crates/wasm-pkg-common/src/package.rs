use std::str::FromStr;

use serde::{Deserialize, Serialize};

pub use semver::Version;

use crate::{label::Label, Error};

/// A package reference, consisting of kebab-case namespace and name.
///
/// Ex: `wasm-pkg:client`
#[derive(Clone, Debug, PartialEq, Eq, Hash, Ord, PartialOrd, Serialize, Deserialize)]
#[serde(into = "String", try_from = "String")]
pub struct PackageRef {
    namespace: Label,
    name: Label,
}

impl PackageRef {
    /// Create a new package reference from a namespace and name.
    pub fn new(namespace: Label, name: Label) -> Self {
        Self { namespace, name }
    }

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

impl FromStr for PackageRef {
    type Err = Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        s.to_string().try_into()
    }
}

/// A package spec combines a [`PackageRef`] with an optional version.
#[derive(Clone, Debug)]
pub struct PackageSpec {
    pub package: PackageRef,
    pub version: Option<Version>,
}

impl FromStr for PackageSpec {
    type Err = Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let (package, version) = s
            .split_once('@')
            .map(|(pkg, ver)| (pkg, Some(ver)))
            .unwrap_or((s, None));
        Ok(Self {
            package: package.parse()?,
            version: version.map(|ver| ver.parse()).transpose()?,
        })
    }
}
