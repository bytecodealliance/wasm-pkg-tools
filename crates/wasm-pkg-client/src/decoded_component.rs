use crate::{ContentStream, PublishingSource};
use futures_util::TryStreamExt;
use std::io::Read;
use tokio::io::AsyncSeekExt;
use tokio_util::io::{StreamReader, SyncIoBridge};
use wasm_pkg_common::{
    Error,
    package::{PackageRef, Version},
};
use wit_component::DecodedWasm;

pub struct DecodedComponent {
    version: Version,
    package_ref: PackageRef,
    decoded_wasm: DecodedWasm,
}

impl DecodedComponent {
    pub async fn from_publishing_source(
        data: PublishingSource,
    ) -> Result<(PublishingSource, DecodedComponent), Error> {
        let (reader, decoded_wasm) = decode(SyncIoBridge::new(data)).await?;
        let (package_ref, version) = extract_package_version(&decoded_wasm)?;

        let mut data = reader.into_inner();
        data.rewind().await?;

        Ok((
            data,
            DecodedComponent {
                version,
                package_ref,
                decoded_wasm,
            },
        ))
    }

    /// Like [`Self::from_publishing_source`] but overrides the derived
    /// `(package, version)` identity with `package_override` when supplied.
    pub async fn from_publishing_source_with_package(
        data: PublishingSource,
        package_override: Option<(PackageRef, Version)>,
    ) -> Result<(PublishingSource, DecodedComponent), Error> {
        let (data, mut decoded) = Self::from_publishing_source(data).await?;
        if let Some((p, v)) = package_override {
            decoded.package_ref = p;
            decoded.version = v;
        }
        Ok((data, decoded))
    }

    /// Construct from a registry content stream. Callers already know the
    /// `(package, version)` identity from the registry listing they followed
    /// to get here, so we take it as input rather than re-deriving it from
    /// the wasm metadata.
    pub async fn from_content_stream(
        stream: ContentStream,
        package_ref: PackageRef,
        version: Version,
    ) -> Result<DecodedComponent, Error> {
        let reader = SyncIoBridge::new(StreamReader::new(stream.map_err(std::io::Error::other)));
        let (_reader, decoded_wasm) = decode(reader).await?;
        Ok(DecodedComponent {
            version,
            package_ref,
            decoded_wasm,
        })
    }

    pub fn version(&self) -> &Version {
        &self.version
    }

    pub fn package(&self) -> &PackageRef {
        &self.package_ref
    }

    /// Check that `self` and `other` are semver-compatible neighbors in the
    /// same cargo-`^` compatibility range.
    pub fn semver_check(&self, other: &DecodedComponent) -> Result<(), Error> {
        // `wit_component::semver_check` is asymmetric: its `new` may add
        // imports / drop exports relative to its `prev`. To get a symmetric
        // additive-only gate between two published versions we pass the
        // newer-in-time release as `prev` and the older as `new`.
        let (older, newer) = if self.version < other.version {
            (self, other)
        } else {
            (other, self)
        };

        let (prev_resolve, prev_world) = extract_resolve_and_world_id(&newer.decoded_wasm)?;
        let (new_resolve, new_world) = extract_resolve_and_world_id(&older.decoded_wasm)?;

        // Merge resolves, remap merged resolve, check for incompatibility
        let mut merged = prev_resolve.clone();
        let new_world = merged
            .merge(new_resolve.clone())
            .and_then(|remap| remap.map_world(new_world, None))
            .map_err(Error::InvalidComponent)?;

        wit_component::semver_check(merged, prev_world, new_world).map_err(|e| {
            Error::SemverIncompatible {
                previous: older.version.clone(),
                new: newer.version.clone(),
                source: e,
            }
        })
    }
}

impl PartialEq for DecodedComponent {
    fn eq(&self, other: &Self) -> bool {
        self.package_ref == other.package_ref && self.version == other.version
    }
}

impl Eq for DecodedComponent {}

impl PartialOrd for DecodedComponent {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for DecodedComponent {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        (&self.package_ref, &self.version).cmp(&(&other.package_ref, &other.version))
    }
}

async fn decode<R>(reader: R) -> Result<(R, DecodedWasm), Error>
where
    R: Read + Send + 'static,
{
    // wit_component::decode_reader is CPU-bound sync work
    // run it on the blocking pool so we don't stall an async worker thread
    // see also: https://docs.rs/tokio/latest/tokio/index.html#cpu-bound-tasks-and-blocking-code
    tokio::task::spawn_blocking(move || {
        let mut reader = reader;
        let decoded_wasm =
            wit_component::decode_reader(&mut reader).map_err(Error::InvalidComponent)?;
        Ok::<_, Error>((reader, decoded_wasm))
    })
    .await
    .map_err(|e| Error::IoError(std::io::Error::other(e)))?
}

/// Extract the package name and version from a decoded candidate.
fn extract_package_version(decoded: &DecodedWasm) -> Result<(PackageRef, Version), Error> {
    let resolve = decoded.resolve();
    let package_id = match decoded {
        wit_component::DecodedWasm::Component(_, world_id) => {
            resolve.worlds[*world_id].package.ok_or_else(|| {
                crate::Error::InvalidComponent(anyhow::anyhow!(
                    "component world or package not found"
                ))
            })?
        }
        wit_component::DecodedWasm::WitPackage(_, pkg) => *pkg,
    };
    let (package, version) = resolve
        .package_names
        .iter()
        .find_map(|(pkg, id)| {
            // SAFETY: We just parsed this from wit and should be able to unwrap. If it
            // isn't a valid identifier, something else is majorly wrong
            (*id == package_id).then(|| {
                (
                    PackageRef::new(
                        pkg.namespace.clone().try_into().unwrap(),
                        pkg.name.clone().try_into().unwrap(),
                    ),
                    pkg.version.clone(),
                )
            })
        })
        .ok_or_else(|| {
            crate::Error::InvalidComponent(anyhow::anyhow!(
                "component package {package_id:?} not found"
            ))
        })?;

    let version = version.ok_or_else(|| {
        crate::Error::InvalidComponent(anyhow::anyhow!(
            "component package version not found in the Wasm binary\n\
            \n\
            The Wasm file was built without a version in the WIT `package` statement.\n\
            Add a version to the `package` statement in your .wit file, e.g.:\n\
            \n\
            \tpackage example:my-package@1.0.0;\n\
            \n\
            Alternatively, specify the package and version explicitly with the --package flag:\n\
            \n\
            \twkg publish <file> --package <namespace>:<name>@<version>"
        ))
    })?;
    Ok((package, version))
}

/// Borrow the inner `wit_parser::Resolve` and resolve a concrete `WorldId`.
/// For a decoded component the world is fixed; for a WIT package we ask
/// `Resolve::select_world` to pick one — deferred until needed so a
/// multi-world WIT package can publish its first version unambiguously.
fn extract_resolve_and_world_id(
    decoded: &DecodedWasm,
) -> Result<(&wit_parser::Resolve, wit_parser::WorldId), Error> {
    match decoded {
        DecodedWasm::Component(resolve, world_id) => Ok((resolve, *world_id)),
        DecodedWasm::WitPackage(resolve, pkg) => {
            let world_id = resolve
                .select_world(&[*pkg], None)
                .map_err(Error::InvalidPackage)?;
            Ok((resolve, world_id))
        }
    }
}
