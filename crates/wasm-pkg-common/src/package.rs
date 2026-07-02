use std::str::FromStr;

use serde::{Deserialize, Serialize};

use crate::{Error, label::Label};

pub use semver::Version;

#[cfg(all(feature = "ansi-term-output", not(feature = "test")))]
pub(crate) mod ansi {
    use anstyle::{Ansi256Color, AnsiColor, Style};

    pub(crate) const LABEL: Style = AnsiColor::BrightBlue.on_default().bold();
    pub(crate) const VERSION: Style = AnsiColor::BrightRed.on_default();
    pub(crate) const SEP: Style = Ansi256Color(249).on_default();
}

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
    pub const fn namespace(&self) -> &Label {
        &self.namespace
    }

    /// Returns the name of the package.
    pub fn name(&self) -> &Label {
        &self.name
    }
}

impl std::fmt::Display for PackageRef {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        #[cfg(all(feature = "ansi-term-output", not(feature = "test")))]
        {
            use ansi::{LABEL, SEP};
            write!(
                f,
                "{LABEL}{}{LABEL:#}{SEP}:{SEP:#}{LABEL}{}{LABEL:#}",
                self.namespace, self.name,
            )
        }
        #[cfg(not(all(feature = "ansi-term-output", not(feature = "test"))))]
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
impl std::fmt::Display for PackageSpec {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match &self.version {
            Some(version) => {
                #[cfg(all(feature = "ansi-term-output", not(feature = "test")))]
                {
                    use ansi::{SEP, VERSION};
                    write!(
                        f,
                        "{}{SEP}@{SEP:#}{VERSION}{version}{VERSION:#}",
                        self.package,
                    )
                }
                #[cfg(not(all(feature = "ansi-term-output", not(feature = "test"))))]
                write!(f, "{}@{version}", self.package)
            }
            None => write!(f, "{}", self.package),
        }
    }
}

/// A package spec combines a [`PackageRef`] with an optional version.
#[derive(Clone, Debug, Eq, PartialEq, PartialOrd, Ord)]
pub struct PackageSpec {
    pub package: PackageRef,
    pub version: Option<Version>,
}

impl PartialEq<str> for PackageSpec {
    fn eq(&self, other: &str) -> bool {
        // clippy --fix will create a recursive callsite here if `self.to_string()` is used instead
        format!("{self}") == other
    }
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
