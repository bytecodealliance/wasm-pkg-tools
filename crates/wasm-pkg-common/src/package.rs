use std::{cmp::Ordering, str::FromStr};

use serde::{Deserialize, Serialize};

use crate::{Error, label::Label};

pub use semver::{Comparator, Op, Version, VersionReq};

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
        if let Some(version) = &self.version {
            write!(f, "{}@{}", self.package, version)
        } else {
            write!(f, "{}", self.package)
        }
    }
}

/// A package spec combines a [`PackageRef`] with an optional version.
#[derive(Clone, Debug, Eq, PartialEq, PartialOrd, Ord, Hash)]
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

/// A [`PackageRef`] plus the version range a dependent requires of it.
///
/// A [`PackageSpec`] names one package; a `PackageReq` describes a range, so it can be tested
/// against a spec ([`Self::matches`]) and against another requirement ([`Self::intersects`]).
#[derive(Clone, Debug, Eq, PartialEq, Hash)]
pub struct PackageReq {
    pub package: PackageRef,
    pub version: VersionReq,
}

impl PackageReq {
    /// A requirement for exactly `version` of `package`.
    pub fn exact(package: PackageRef, version: &Version) -> Self {
        let version = format!("={version}")
            .parse()
            .expect("an exact version is always a valid requirement");
        Self { package, version }
    }

    /// A requirement for any version of `package`.
    pub fn any(package: PackageRef) -> Self {
        Self {
            package,
            version: VersionReq::STAR,
        }
    }

    /// Whether this requirement accepts any version (`*`).
    pub fn is_any(&self) -> bool {
        self.version == VersionReq::STAR
    }

    /// The version this requirement names, if it is a single `=x.y.z` comparator.
    pub fn as_exact(&self) -> Option<Version> {
        match self.version.comparators.as_slice() {
            [
                Comparator {
                    op: Op::Exact,
                    major,
                    minor: Some(minor),
                    patch: Some(patch),
                    pre,
                },
            ] => Some(Version {
                major: *major,
                minor: *minor,
                patch: *patch,
                pre: pre.clone(),
                build: Default::default(),
            }),
            _ => None,
        }
    }

    /// Whether `spec` satisfies this requirement.
    ///
    /// A spec with no version is the versionless package, so it only satisfies [`Self::any`].
    pub fn matches(&self, spec: &PackageSpec) -> bool {
        self.package == spec.package
            && match &spec.version {
                Some(version) => self.version.matches(version),
                None => self.is_any(),
            }
    }

    /// Whether this requirement and `other` accept any of the same versions.
    ///
    /// Exact and `*` requirements (all an override key can produce) compare precisely; two general
    /// ranges fall back to sampling each comparator's version.
    pub fn intersects(&self, other: &Self) -> bool {
        if self.package != other.package {
            return false;
        }
        if self.is_any() || other.is_any() {
            return true;
        }
        if let Some(version) = self.as_exact() {
            return other.version.matches(&version);
        }
        if let Some(version) = other.as_exact() {
            return self.version.matches(&version);
        }
        self.version
            .comparators
            .iter()
            .chain(other.version.comparators.iter())
            .flat_map(comparator_samples)
            .any(|version| self.version.matches(&version) && other.version.matches(&version))
    }
}

/// Version candidates named by a comparator, plus the next patch so a strict bound has a sample
/// inside its range.
fn comparator_samples(comparator: &Comparator) -> impl Iterator<Item = Version> {
    let base = Version {
        major: comparator.major,
        minor: comparator.minor.unwrap_or(0),
        patch: comparator.patch.unwrap_or(0),
        pre: comparator.pre.clone(),
        build: Default::default(),
    };
    let mut next = base.clone();
    next.patch += 1;
    [base, next].into_iter()
}

impl From<PackageSpec> for PackageReq {
    fn from(spec: PackageSpec) -> Self {
        match spec.version {
            Some(version) => Self::exact(spec.package, &version),
            None => Self::any(spec.package),
        }
    }
}

impl From<&PackageSpec> for PackageReq {
    fn from(spec: &PackageSpec) -> Self {
        spec.clone().into()
    }
}

impl std::fmt::Display for PackageReq {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}@{}", self.package, self.version)
    }
}

impl FromStr for PackageReq {
    type Err = Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let (package, version) = s
            .split_once('@')
            .ok_or_else(|| Error::InvalidPackageRef(format!("missing expected '@' in `{s}`")))?;
        Ok(Self {
            package: package.parse()?,
            version: version
                .parse()
                .map_err(|e| Error::InvalidPackageRef(format!("invalid version in `{s}`: {e}")))?,
        })
    }
}

// `VersionReq` has no `Ord`, so order by the package then the rendered requirement.
impl Ord for PackageReq {
    fn cmp(&self, other: &Self) -> Ordering {
        self.package
            .cmp(&other.package)
            .then_with(|| self.version.to_string().cmp(&other.version.to_string()))
    }
}

impl PartialOrd for PackageReq {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pkg() -> PackageRef {
        "foo:bar".parse().unwrap()
    }

    fn other() -> PackageRef {
        "foo:baz".parse().unwrap()
    }

    fn exact(version: &str) -> PackageReq {
        PackageReq::exact(pkg(), &version.parse().unwrap())
    }

    fn spec(version: Option<&str>) -> PackageSpec {
        PackageSpec {
            package: pkg(),
            version: version.map(|v| v.parse().unwrap()),
        }
    }

    #[test]
    fn package_req_parse_and_display_roundtrip() {
        for text in [
            "foo:bar@*",
            "foo:bar@=0.1.0",
            "foo:bar@^1.2",
            "foo:bar@>=1, <2",
        ] {
            let req: PackageReq = text.parse().unwrap();
            assert_eq!(req.to_string(), text, "roundtrip for `{text}`");
        }

        assert!("foo:bar".parse::<PackageReq>().is_err());
    }

    #[test]
    fn package_req_from_spec() {
        assert!(PackageReq::from(spec(None)).is_any());
        assert_eq!(PackageReq::from(spec(Some("0.1.0"))), exact("0.1.0"));
    }

    #[test]
    fn matches_versioned_and_versionless_specs() {
        // A versionless spec only satisfies `*`.
        assert!(PackageReq::any(pkg()).matches(&spec(None)));
        assert!(!exact("0.1.0").matches(&spec(None)));

        assert!(PackageReq::any(pkg()).matches(&spec(Some("1.2.3"))));
        assert!(exact("0.1.0").matches(&spec(Some("0.1.0"))));
        assert!(!exact("0.1.0").matches(&spec(Some("0.2.0"))));
        assert!(!PackageReq::any(other()).matches(&spec(None)));
    }

    #[test]
    fn intersects_exact_and_any() {
        assert!(PackageReq::any(pkg()).intersects(&PackageReq::any(pkg())));
        assert!(PackageReq::any(pkg()).intersects(&exact("0.1.0")));
        assert!(exact("0.1.0").intersects(&PackageReq::any(pkg())));

        assert!(!exact("0.1.0").intersects(&exact("0.2.0")));
        assert!(exact("0.1.0").intersects(&exact("0.1.0")));

        assert!(!exact("0.1.0").intersects(&PackageReq::exact(other(), &"0.1.0".parse().unwrap())));
    }

    #[test]
    fn intersects_exact_and_range() {
        let range: PackageReq = "foo:bar@^0.1".parse().unwrap();
        assert!(range.intersects(&exact("0.1.5")));
        assert!(!range.intersects(&exact("0.2.0")));
        assert!(exact("0.1.5").intersects(&range));
        assert!(!exact("0.2.0").intersects(&range));
    }

    #[test]
    fn ordering_is_deterministic() {
        let mut reqs = [exact("0.2.0"), PackageReq::any(pkg()), exact("0.1.0")];
        reqs.sort();
        let rendered: Vec<String> = reqs.iter().map(ToString::to_string).collect();
        assert_eq!(
            rendered,
            ["foo:bar@*", "foo:bar@=0.1.0", "foo:bar@=0.2.0"],
            "requirements should sort by package then rendered range"
        );
    }
}
