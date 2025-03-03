//! Type definitions and functions for working with `wkg.lock` files.

use std::{
    cmp::Ordering,
    collections::{BTreeSet, HashMap},
    ops::{Deref, DerefMut},
    path::{Path, PathBuf},
};

use anyhow::{Context, Result};
use semver::VersionReq;
use serde::{Deserialize, Serialize};
use tokio::{
    fs::{File, OpenOptions},
    io::{AsyncReadExt, AsyncSeekExt, AsyncWriteExt},
};
use wasm_pkg_client::{ContentDigest, PackageRef, Version};

use crate::resolver::{DependencyResolution, DependencyResolutionMap};

/// The default name of the lock file.
pub const LOCK_FILE_NAME: &str = "wkg.lock";
/// The version of the lock file for v1
pub const LOCK_FILE_V1: u64 = 1;

/// Represents a resolved dependency lock file.
///
/// This is a TOML file that contains the resolved dependency information from
/// a previous build.
#[derive(Debug, serde::Serialize)]
pub struct LockFile {
    /// The version of the lock file.
    ///
    /// Currently this is always `1`.
    pub version: u64,

    /// The locked dependencies in the lock file.
    ///
    /// This list is sorted by the name of the locked package.
    pub packages: BTreeSet<LockedPackage>,

    #[serde(skip)]
    locker: Locker,
}

impl PartialEq for LockFile {
    fn eq(&self, other: &Self) -> bool {
        self.packages == other.packages && self.version == other.version
    }
}

impl Eq for LockFile {}

impl LockFile {
    /// Creates a new lock file from the given packages at the given path. This will create an empty
    /// file and get an exclusive lock on the file, but will not write the data to the file unless
    /// [`write`](Self::write) is called.
    pub async fn new_with_path(
        packages: impl IntoIterator<Item = LockedPackage>,
        path: impl AsRef<Path>,
    ) -> Result<Self> {
        let locker = Locker::open_rw(path.as_ref()).await?;
        Ok(Self {
            version: LOCK_FILE_V1,
            packages: packages.into_iter().collect(),
            locker,
        })
    }

    /// Loads a lock file from the given path. If readonly is set to false, then an exclusive lock
    /// will be acquired on the file. This function will block until the lock is acquired.
    pub async fn load_from_path(path: impl AsRef<Path>, readonly: bool) -> Result<Self> {
        let mut locker = if readonly {
            Locker::open_ro(path.as_ref()).await
        } else {
            Locker::open_rw(path.as_ref()).await
        }?;
        let mut contents = String::new();
        locker
            .read_to_string(&mut contents)
            .await
            .context("unable to load lock file from path")?;
        let lock_file: LockFileIntermediate =
            toml::from_str(&contents).context("unable to parse lock file from path")?;
        // Ensure version is correct and error if it isn't
        if lock_file.version != LOCK_FILE_V1 {
            return Err(anyhow::anyhow!(
                "unsupported lock file version: {}",
                lock_file.version
            ));
        }
        // Rewind the file after reading just to be safe. We already do this before writing, but
        // just in case we add any future logic, we can reset the file here so as to not cause
        // issues
        locker
            .rewind()
            .await
            .context("Unable to reset file after reading")?;
        Ok(lock_file.into_lock_file(locker))
    }

    /// Creates a lock file from the dependency map. This will create an empty file (if it doesn't
    /// exist) and get an exclusive lock on the file, but will not write the data to the file unless
    /// [`write`](Self::write) is called.
    pub async fn from_dependencies(
        map: &DependencyResolutionMap,
        path: impl AsRef<Path>,
    ) -> Result<LockFile> {
        let packages = generate_locked_packages(map);

        LockFile::new_with_path(packages, path).await
    }

    /// A helper for updating the current lock file with the given dependency map. This will clear current
    /// packages that are not in the dependency map and add new packages that are in the dependency
    /// map.
    ///
    /// This function will not write the data to the file unless [`write`](Self::write) is called.
    pub fn update_dependencies(&mut self, map: &DependencyResolutionMap) {
        self.packages.clear();
        self.packages.extend(generate_locked_packages(map));
    }

    /// Attempts to load the lock file from the current directory. Most of the time, users of this
    /// crate should use this function. Right now it just checks for a `wkg.lock` file in the
    /// current directory, but we could add more resolution logic in the future. If the file is not
    /// found, a new file is created and a default empty lockfile is returned. This function will
    /// block until the lock is acquired.
    pub async fn load(readonly: bool) -> Result<Self> {
        let lock_path = PathBuf::from(LOCK_FILE_NAME);
        if !tokio::fs::try_exists(&lock_path).await? {
            // Create a new lock file if it doesn't exist so we can then open it readonly if that is set
            let mut temp_lock = Self::new_with_path([], &lock_path).await?;
            temp_lock.write().await?;
        }
        Self::load_from_path(lock_path, readonly).await
    }

    /// Serializes and writes the lock file
    pub async fn write(&mut self) -> Result<()> {
        let contents = toml::to_string_pretty(self)?;
        // Truncate the file before writing to it
        self.locker.rewind().await.with_context(|| {
            format!(
                "unable to rewind lock file at path {}",
                self.locker.path.display()
            )
        })?;
        self.locker.set_len(0).await.with_context(|| {
            format!(
                "unable to truncate lock file at path {}",
                self.locker.path.display()
            )
        })?;

        self.locker.write_all(
            b"# This file is automatically generated.\n# It is not intended for manual editing.\n",
        )
        .await.with_context(|| format!("unable to write lock file to path {}", self.locker.path.display()))?;
        self.locker
            .write_all(contents.as_bytes())
            .await
            .with_context(|| {
                format!(
                    "unable to write lock file to path {}",
                    self.locker.path.display()
                )
            })?;
        // Make sure to flush and sync just to be sure the file doesn't drop and the lock is
        // released too early
        self.locker.sync_all().await.with_context(|| {
            format!(
                "unable to write lock file to path {}",
                self.locker.path.display()
            )
        })
    }

    /// Resolves a package from the lock file.
    ///
    /// Returns `Ok(None)` if the package cannot be resolved.
    ///
    /// Fails if the package cannot be resolved and the lock file is not allowed to be updated.
    pub fn resolve(
        &self,
        registry: Option<&str>,
        package_ref: &PackageRef,
        requirement: &VersionReq,
    ) -> Result<Option<&LockedPackageVersion>> {
        // NOTE(thomastaylor312): Using a btree map so we don't have to keep sorting the vec. The
        // tradeoff is we have to clone two things here to do the fetch. That tradeoff seems fine to
        // me, especially because this is used in CLI commands.
        if let Some(pkg) = self.packages.get(&LockedPackage {
            name: package_ref.clone(),
            registry: registry.map(ToString::to_string),
            versions: vec![],
        }) {
            if let Some(locked) = pkg
                .versions
                .iter()
                .find(|locked| &locked.requirement == requirement)
            {
                tracing::info!(%package_ref, ?registry, %requirement, resolved_version = %locked.version, "dependency package was resolved by the lock file");
                return Ok(Some(locked));
            }
        }

        tracing::info!(%package_ref, ?registry, %requirement, "dependency package was not in the lock file");
        Ok(None)
    }
}

fn generate_locked_packages(map: &DependencyResolutionMap) -> impl Iterator<Item = LockedPackage> {
    type PackageKey = (PackageRef, Option<String>);
    type VersionsMap = HashMap<String, (Version, ContentDigest)>;
    let mut packages: HashMap<PackageKey, VersionsMap> = HashMap::new();

    for resolution in map.values() {
        match resolution.key() {
            Some((id, registry)) => {
                let pkg = match resolution {
                    DependencyResolution::Registry(pkg) => pkg,
                    DependencyResolution::Local(_) => unreachable!(),
                };

                let prev = packages
                    .entry((id.clone(), registry.map(str::to_string)))
                    .or_default()
                    .insert(
                        pkg.requirement.to_string(),
                        (pkg.version.clone(), pkg.digest.clone()),
                    );

                if let Some((prev, _)) = prev {
                    // The same requirements should resolve to the same version
                    assert!(prev == pkg.version)
                }
            }
            None => continue,
        }
    }

    packages.into_iter().map(|((name, registry), versions)| {
        let versions: Vec<LockedPackageVersion> = versions
            .into_iter()
            .map(|(requirement, (version, digest))| LockedPackageVersion {
                requirement: requirement
                    .parse()
                    .expect("Version requirement should have been valid. This is programmer error"),
                version,
                digest,
            })
            .collect();

        LockedPackage {
            name,
            registry,
            versions,
        }
    })
}

/// Represents a locked package in a lock file.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct LockedPackage {
    /// The name of the locked package.
    pub name: PackageRef,

    /// The registry the package was resolved from.
    // NOTE(thomastaylor312): This is a string instead of using the `Registry` type because clippy
    // is complaining about it being an interior mutable key type for the btreeset
    pub registry: Option<String>,

    /// The locked version of a package.
    ///
    /// A package may have multiple locked versions if more than one
    /// version requirement was specified for the package in `wit.toml`.
    #[serde(alias = "version", default, skip_serializing_if = "Vec::is_empty")]
    pub versions: Vec<LockedPackageVersion>,
}

impl Ord for LockedPackage {
    fn cmp(&self, other: &Self) -> Ordering {
        if self.name == other.name {
            self.registry.cmp(&other.registry)
        } else {
            self.name.cmp(&other.name)
        }
    }
}

impl PartialOrd for LockedPackage {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

/// Represents version information for a locked package.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct LockedPackageVersion {
    /// The version requirement used to resolve this version
    pub requirement: VersionReq,
    /// The version the package is locked to.
    pub version: Version,
    /// The digest of the package contents.
    pub digest: ContentDigest,
}

#[derive(Debug, Deserialize)]
struct LockFileIntermediate {
    version: u64,

    #[serde(alias = "package", default, skip_serializing_if = "Vec::is_empty")]
    packages: BTreeSet<LockedPackage>,
}

impl LockFileIntermediate {
    fn into_lock_file(self, locker: Locker) -> LockFile {
        LockFile {
            version: self.version,
            packages: self.packages,
            locker,
        }
    }
}

/// Used to indicate the access mode of a lock file.
#[derive(Debug, Copy, Clone, Eq, PartialEq)]
enum Access {
    Shared,
    Exclusive,
}

/// A wrapper around a lockable file
#[derive(Debug)]
struct Locker {
    file: File,
    path: PathBuf,
}

impl Drop for Locker {
    fn drop(&mut self) {
        let _ = sys::unlock(&self.file);
    }
}

impl Deref for Locker {
    type Target = File;

    fn deref(&self) -> &Self::Target {
        &self.file
    }
}

impl DerefMut for Locker {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.file
    }
}

impl AsRef<File> for Locker {
    fn as_ref(&self) -> &File {
        &self.file
    }
}

// NOTE(thomastaylor312): These lock file primitives from here on down are mostly copyed wholesale
// from the lock file implementation of cargo-component with some minor modifications to make them
// work with tokio

impl Locker {
    // NOTE(thomastaylor312): I am keeping around these try methods for possible later use. Right
    // now we're ignoring the dead code
    #[allow(dead_code)]
    /// Attempts to acquire exclusive access to a file, returning the locked
    /// version of a file.
    ///
    /// This function will create a file at `path` if it doesn't already exist
    /// (including intermediate directories), and then it will try to acquire an
    /// exclusive lock on `path`.
    ///
    /// If the lock cannot be immediately acquired, `Ok(None)` is returned.
    ///
    /// The returned file can be accessed to look at the path and also has
    /// read/write access to the underlying file.
    pub async fn try_open_rw(path: impl Into<PathBuf>) -> Result<Option<Self>> {
        Self::open(
            path.into(),
            OpenOptions::new().read(true).write(true).create(true),
            Access::Exclusive,
            true,
        )
        .await
    }

    /// Opens exclusive access to a file, returning the locked version of a
    /// file.
    ///
    /// This function will create a file at `path` if it doesn't already exist
    /// (including intermediate directories), and then it will acquire an
    /// exclusive lock on `path`.
    ///
    /// If the lock cannot be acquired, this function will block until it is
    /// acquired.
    ///
    /// The returned file can be accessed to look at the path and also has
    /// read/write access to the underlying file.
    pub async fn open_rw(path: impl Into<PathBuf>) -> Result<Self> {
        Ok(Self::open(
            path.into(),
            OpenOptions::new().read(true).write(true).create(true),
            Access::Exclusive,
            false,
        )
        .await?
        .unwrap())
    }

    #[allow(dead_code)]
    /// Attempts to acquire shared access to a file, returning the locked version
    /// of a file.
    ///
    /// This function will fail if `path` doesn't already exist, but if it does
    /// then it will acquire a shared lock on `path`.
    ///
    /// If the lock cannot be immediately acquired, `Ok(None)` is returned.
    ///
    /// The returned file can be accessed to look at the path and also has read
    /// access to the underlying file. Any writes to the file will return an
    /// error.
    pub async fn try_open_ro(path: impl Into<PathBuf>) -> Result<Option<Self>> {
        Self::open(
            path.into(),
            OpenOptions::new().read(true),
            Access::Shared,
            true,
        )
        .await
    }

    /// Opens shared access to a file, returning the locked version of a file.
    ///
    /// This function will fail if `path` doesn't already exist, but if it does
    /// then it will acquire a shared lock on `path`.
    ///
    /// If the lock cannot be acquired, this function will block until it is
    /// acquired.
    ///
    /// The returned file can be accessed to look at the path and also has read
    /// access to the underlying file. Any writes to the file will return an
    /// error.
    pub async fn open_ro(path: impl Into<PathBuf>) -> Result<Self> {
        Ok(Self::open(
            path.into(),
            OpenOptions::new().read(true),
            Access::Shared,
            false,
        )
        .await?
        .unwrap())
    }

    async fn open(
        path: PathBuf,
        opts: &OpenOptions,
        access: Access,
        try_lock: bool,
    ) -> Result<Option<Self>> {
        // If we want an exclusive lock then if we fail because of NotFound it's
        // likely because an intermediate directory didn't exist, so try to
        // create the directory and then continue.
        let file = match opts.open(&path).await {
            Ok(file) => Ok(file),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound && access == Access::Exclusive => {
                tokio::fs::create_dir_all(path.parent().unwrap())
                    .await
                    .with_context(|| {
                        format!(
                            "failed to create parent directories for `{path}`",
                            path = path.display()
                        )
                    })?;
                opts.open(&path).await
            }
            Err(e) => Err(e),
        }
        .with_context(|| format!("failed to open `{path}`", path = path.display()))?;

        // Now that the file exists, canonicalize the path for better debuggability.
        let path = tokio::fs::canonicalize(path)
            .await
            .context("failed to canonicalize path")?;
        let mut lock = Self { file, path };

        // File locking on Unix is currently implemented via `flock`, which is known
        // to be broken on NFS. We could in theory just ignore errors that happen on
        // NFS, but apparently the failure mode [1] for `flock` on NFS is **blocking
        // forever**, even if the "non-blocking" flag is passed!
        //
        // As a result, we just skip all file locks entirely on NFS mounts. That
        // should avoid calling any `flock` functions at all, and it wouldn't work
        // there anyway.
        //
        // [1]: https://github.com/rust-lang/cargo/issues/2615
        if is_on_nfs_mount(&lock.path) {
            return Ok(Some(lock));
        }

        let res = match (access, try_lock) {
            (Access::Shared, true) => sys::try_lock_shared(&lock.file),
            (Access::Exclusive, true) => sys::try_lock_exclusive(&lock.file),
            (Access::Shared, false) => {
                // We have to move the lock into the thread because it requires exclusive ownership
                // for dropping. We return it back out after the blocking IO.
                let (l, res) = tokio::task::spawn_blocking(move || {
                    let res = sys::lock_shared(&lock.file);
                    (lock, res)
                })
                .await
                .context("error waiting for blocking IO")?;
                lock = l;
                res
            }
            (Access::Exclusive, false) => {
                // We have to move the lock into the thread because it requires exclusive ownership
                // for dropping. We return it back out after the blocking IO.
                let (l, res) = tokio::task::spawn_blocking(move || {
                    let res = sys::lock_exclusive(&lock.file);
                    (lock, res)
                })
                .await
                .context("error waiting for blocking IO")?;
                lock = l;
                res
            }
        };

        return match res {
            Ok(_) => Ok(Some(lock)),

            // In addition to ignoring NFS which is commonly not working we also
            // just ignore locking on file systems that look like they don't
            // implement file locking.
            Err(e) if sys::error_unsupported(&e) => Ok(Some(lock)),

            // Check to see if it was a contention error
            Err(e) if try_lock && sys::error_contended(&e) => Ok(None),

            Err(e) => Err(anyhow::anyhow!(e).context(format!(
                "failed to lock file `{path}`",
                path = lock.path.display()
            ))),
        };

        #[cfg(all(target_os = "linux", not(target_env = "musl")))]
        fn is_on_nfs_mount(path: &Path) -> bool {
            use std::ffi::CString;
            use std::mem;
            use std::os::unix::prelude::*;

            let path = match CString::new(path.as_os_str().as_bytes()) {
                Ok(path) => path,
                Err(_) => return false,
            };

            unsafe {
                let mut buf: libc::statfs = mem::zeroed();
                let r = libc::statfs(path.as_ptr(), &mut buf);

                r == 0 && buf.f_type as u32 == libc::NFS_SUPER_MAGIC as u32
            }
        }

        #[cfg(any(not(target_os = "linux"), target_env = "musl"))]
        fn is_on_nfs_mount(_path: &Path) -> bool {
            false
        }
    }
}

#[cfg(unix)]
mod sys {
    use std::io::{Error, Result};
    use std::os::unix::io::AsRawFd;

    use tokio::fs::File;

    pub(super) fn lock_shared(file: &File) -> Result<()> {
        flock(file, libc::LOCK_SH)
    }

    pub(super) fn lock_exclusive(file: &File) -> Result<()> {
        flock(file, libc::LOCK_EX)
    }

    pub(super) fn try_lock_shared(file: &File) -> Result<()> {
        flock(file, libc::LOCK_SH | libc::LOCK_NB)
    }

    pub(super) fn try_lock_exclusive(file: &File) -> Result<()> {
        flock(file, libc::LOCK_EX | libc::LOCK_NB)
    }

    pub(super) fn unlock(file: &File) -> Result<()> {
        flock(file, libc::LOCK_UN)
    }

    pub(super) fn error_contended(err: &Error) -> bool {
        err.raw_os_error() == Some(libc::EWOULDBLOCK)
    }

    pub(super) fn error_unsupported(err: &Error) -> bool {
        match err.raw_os_error() {
            // Unfortunately, depending on the target, these may or may not be the same.
            // For targets in which they are the same, the duplicate pattern causes a warning.
            #[allow(unreachable_patterns)]
            Some(libc::ENOTSUP | libc::EOPNOTSUPP) => true,
            Some(libc::ENOSYS) => true,
            _ => false,
        }
    }

    #[cfg(not(target_os = "solaris"))]
    fn flock(file: &File, flag: libc::c_int) -> Result<()> {
        let ret = unsafe { libc::flock(file.as_raw_fd(), flag) };
        if ret < 0 {
            Err(Error::last_os_error())
        } else {
            Ok(())
        }
    }

    #[cfg(target_os = "solaris")]
    fn flock(file: &File, flag: libc::c_int) -> Result<()> {
        // Solaris lacks flock(), so try to emulate using fcntl()
        let mut flock = libc::flock {
            l_type: 0,
            l_whence: 0,
            l_start: 0,
            l_len: 0,
            l_sysid: 0,
            l_pid: 0,
            l_pad: [0, 0, 0, 0],
        };
        flock.l_type = if flag & libc::LOCK_UN != 0 {
            libc::F_UNLCK
        } else if flag & libc::LOCK_EX != 0 {
            libc::F_WRLCK
        } else if flag & libc::LOCK_SH != 0 {
            libc::F_RDLCK
        } else {
            panic!("unexpected flock() operation")
        };

        let mut cmd = libc::F_SETLKW;
        if (flag & libc::LOCK_NB) != 0 {
            cmd = libc::F_SETLK;
        }

        let ret = unsafe { libc::fcntl(file.as_raw_fd(), cmd, &flock) };

        if ret < 0 {
            Err(Error::last_os_error())
        } else {
            Ok(())
        }
    }
}

#[cfg(windows)]
mod sys {
    use std::io::{Error, Result};
    use std::mem;
    use std::os::windows::io::AsRawHandle;

    use tokio::fs::File;
    use windows_sys::Win32::Foundation::HANDLE;
    use windows_sys::Win32::Foundation::{ERROR_INVALID_FUNCTION, ERROR_LOCK_VIOLATION};
    use windows_sys::Win32::Storage::FileSystem::{
        LockFileEx, UnlockFile, LOCKFILE_EXCLUSIVE_LOCK, LOCKFILE_FAIL_IMMEDIATELY,
    };

    pub(super) fn lock_shared(file: &File) -> Result<()> {
        lock_file(file, 0)
    }

    pub(super) fn lock_exclusive(file: &File) -> Result<()> {
        lock_file(file, LOCKFILE_EXCLUSIVE_LOCK)
    }

    pub(super) fn try_lock_shared(file: &File) -> Result<()> {
        lock_file(file, LOCKFILE_FAIL_IMMEDIATELY)
    }

    pub(super) fn try_lock_exclusive(file: &File) -> Result<()> {
        lock_file(file, LOCKFILE_EXCLUSIVE_LOCK | LOCKFILE_FAIL_IMMEDIATELY)
    }

    pub(super) fn error_contended(err: &Error) -> bool {
        err.raw_os_error()
            .map_or(false, |x| x == ERROR_LOCK_VIOLATION as i32)
    }

    pub(super) fn error_unsupported(err: &Error) -> bool {
        err.raw_os_error()
            .map_or(false, |x| x == ERROR_INVALID_FUNCTION as i32)
    }

    pub(super) fn unlock(file: &File) -> Result<()> {
        unsafe {
            let ret = UnlockFile(file.as_raw_handle() as HANDLE, 0, 0, !0, !0);
            if ret == 0 {
                Err(Error::last_os_error())
            } else {
                Ok(())
            }
        }
    }

    fn lock_file(file: &File, flags: u32) -> Result<()> {
        unsafe {
            let mut overlapped = mem::zeroed();
            let ret = LockFileEx(
                file.as_raw_handle() as HANDLE,
                flags,
                0,
                !0,
                !0,
                &mut overlapped,
            );
            if ret == 0 {
                Err(Error::last_os_error())
            } else {
                Ok(())
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use sha2::Digest;

    use super::*;

    #[tokio::test]
    async fn test_shared_locking() {
        let tempdir = tempfile::tempdir().expect("failed to create tempdir");
        let path = tempdir.path().join("test");

        tokio::fs::write(&path, "")
            .await
            .expect("failed to write empty file");

        let _locker1 = Locker::open_ro(path.clone())
            .await
            .expect("failed to open reader locker");
        let _locker2 = Locker::open_ro(path.clone())
            .await
            .expect("should be able to open a second reader");
    }

    #[tokio::test]
    async fn test_exclusive_locking() {
        let tempdir = tempfile::tempdir().expect("failed to create tempdir");
        let path = tempdir.path().join("test");

        tokio::fs::write(&path, "")
            .await
            .expect("failed to write empty file");

        let locker1 = Locker::open_rw(path.clone())
            .await
            .expect("failed to open writer locker");
        let maybe_locker = Locker::try_open_rw(path.clone())
            .await
            .expect("shouldn't fail with a try open");
        assert!(
            maybe_locker.is_none(),
            "Shouldn't be able to open a second writer"
        );

        let maybe_locker = Locker::try_open_ro(path.clone())
            .await
            .expect("shouldn't fail with a try open");
        assert!(maybe_locker.is_none(), "Shouldn't be able to open a reader");

        // A call to open_rw should block until the first locker is dropped
        let (tx, rx) = tokio::sync::oneshot::channel();
        tokio::spawn(async move {
            let res = Locker::open_rw(path.clone()).await;
            tx.send(res).expect("failed to send signal");
        });

        // Sleep here to simulate another process finishing a write
        tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;
        drop(locker1);

        tokio::select! {
            res = rx => {
                assert!(res.is_ok(), "failed to open second write locker");
            }
            _ = tokio::time::sleep(tokio::time::Duration::from_millis(1000)) => {
                panic!("timed out waiting for second locker");
            }
        }
    }

    #[tokio::test]
    async fn test_roundtrip() {
        let tempdir = tempfile::tempdir().expect("failed to create tempdir");
        let path = tempdir.path().join(LOCK_FILE_NAME);

        let mut fakehasher = sha2::Sha256::new();
        fakehasher.update(b"fake");

        let mut expected_deps = BTreeSet::from([
            LockedPackage {
                name: "enterprise:holodeck".parse().unwrap(),
                versions: vec![LockedPackageVersion {
                    version: "0.1.0".parse().unwrap(),
                    digest: fakehasher.clone().into(),
                    requirement: VersionReq::parse("=0.1.0").unwrap(),
                }],
                registry: None,
            },
            LockedPackage {
                name: "ds9:holosuite".parse().unwrap(),
                versions: vec![LockedPackageVersion {
                    version: "0.1.0".parse().unwrap(),
                    digest: fakehasher.clone().into(),
                    requirement: VersionReq::parse("=0.1.0").unwrap(),
                }],
                registry: None,
            },
        ]);

        let mut lock = LockFile::new_with_path(expected_deps.clone(), &path)
            .await
            .expect("Shouldn't fail when creating a new lock file");

        // Write the current file to make sure that works
        lock.write()
            .await
            .expect("Shouldn't fail when writing lock file");

        // Push one more package onto the lock file before writing it
        let new_package = LockedPackage {
            name: "defiant:armor".parse().unwrap(),
            versions: vec![LockedPackageVersion {
                version: "0.1.0".parse().unwrap(),
                digest: fakehasher.into(),
                requirement: VersionReq::parse("=0.1.0").unwrap(),
            }],
            registry: None,
        };

        lock.packages.insert(new_package.clone());
        expected_deps.insert(new_package);

        // Write again with the same file
        lock.write()
            .await
            .expect("Shouldn't fail when writing lock file");

        // Drop the lock file
        drop(lock);

        // Now read the lock file again and make sure everything is correct (and we can lock it
        // properly)
        let lock = LockFile::load_from_path(&path, false)
            .await
            .expect("Shouldn't fail when loading lock file");
        assert_eq!(
            lock.packages, expected_deps,
            "Lock file deps should match expected deps"
        );
        assert_eq!(lock.version, LOCK_FILE_V1, "Lock file version should be 1");
    }
}
