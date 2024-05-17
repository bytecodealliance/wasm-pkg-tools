use std::str::FromStr;

use wasm_pkg_loader::{PackageRef, Version};

// TODO: move to some library crate
#[derive(Clone, Debug)]
pub struct PackageSpec {
    pub package: PackageRef,
    pub version: Option<Version>,
}

impl FromStr for PackageSpec {
    type Err = anyhow::Error;

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
