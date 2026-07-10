use std::collections::{BTreeSet, HashMap};
use std::io::Cursor;
use std::path::PathBuf;

use anyhow::Context;
use wasm_pkg_client::{
    Client, PublishOpts,
    caching::{CachingClient, FileCache},
    local::LocalConfig,
};
use wasm_pkg_common::{
    config::{Config, RegistryConfig, RegistryMapping},
    metadata::LOCAL_PROTOCOL,
    package::PackageRef,
    registry::Registry,
};
use wasm_pkg_core::{lock::LockFile, resolver::PublishPlan};

use crate::wit::build_wit_dir;

/// A [`CachingClient`] and [`PublishPlan`] wired to a temporary local backend
pub(crate) struct PublishVerifier {
    #[expect(dead_code, reason = "workspaces")]
    pub(crate) client: CachingClient<FileCache>,
    pub(crate) plan: PublishPlan,
    #[expect(dead_code, reason = "workspaces")]
    pub(crate) packages: BTreeSet<PackageRef>,
    pub(crate) data: HashMap<PackageRef, Vec<u8>>,
    /// Held so the temp local backend outlives the returned client.
    _local_config: LocalConfig,
}

impl PublishVerifier {
    pub(crate) async fn try_new(
        paths: &[PathBuf],
        registry_name: &str,
        mut base_config: Config,
        cache: FileCache,
        lock_file: &mut LockFile,
        capture_bytes: bool,
    ) -> anyhow::Result<PublishVerifier> {
        let local_config = LocalConfig::temp_dir()?;
        let reg_config =
            RegistryConfig::default().with_default_backend(LOCAL_PROTOCOL, &local_config)?;
        let registry: Registry = registry_name.parse()?;
        base_config
            .get_or_insert_registry_config_mut(&registry)
            .merge(reg_config);

        let plan = PublishPlan::from_paths(paths).context("failed to build publish plan")?;
        let packages: BTreeSet<PackageRef> = plan.iter().map(|spec| spec.package.clone()).collect();

        // TODO(mkatychev): Add support for `PackageLoader::get_release` to handle
        // querying on a per package, namespace, and registry level
        // to handle cargo style overlays.
        // see this reference of `cargo::core::Dependency` usage for local overlays in Cargo:
        // https://github.com/rust-lang/cargo/blob/d6900d00af2644ea1c0068c5694d9dbe11a3ab39/src/cargo/sources/overlay.rs#L47
        for pkg in &packages {
            base_config.set_package_registry_override(
                pkg.clone(),
                RegistryMapping::Registry(registry.clone()),
            );
        }

        let client = CachingClient::new(Some(Client::new(base_config)), cache);

        let mut bytes_by_package = HashMap::new();
        for spec in plan.iter() {
            let path = plan
                .get_path(&spec.package)
                .expect("PublishPlan guarantees a path for each iterated spec");
            let bytes = if path.is_dir() {
                let (_pkg_ref, _version, bytes) =
                    build_wit_dir(path, client.clone(), lock_file).await?;
                bytes
            } else {
                tokio::fs::read(path).await.with_context(|| {
                    format!("failed to read workspace member at {}", path.display())
                })?
            };
            client
                .client()?
                .publish_release_data(
                    Box::pin(Cursor::new(bytes.clone())),
                    PublishOpts {
                        package: None,
                        registry: Some(registry.clone()),
                        dry_run: false,
                        skip_semver_check: false,
                    },
                )
                .await
                .with_context(|| format!("verifier failed to publish: {}", spec.package))?;
            if capture_bytes {
                bytes_by_package.insert(spec.package.clone(), bytes);
            }
        }

        Ok(PublishVerifier {
            client,
            plan,
            packages,
            data: bytes_by_package,
            _local_config: local_config,
        })
    }
}
