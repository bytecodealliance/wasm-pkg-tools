use anyhow::Result;
use std::path::{Component, Path, PathBuf};

use crate::manifest::MANIFEST_FILE_NAME;

/// Find the first ancestor [`super::Manifest`] path for current working directory.
pub(crate) fn find_root_manifest_for_wd(cwd: impl AsRef<Path>) -> Option<PathBuf> {
    for current in cwd.as_ref().ancestors() {
        let manifest = current.join(MANIFEST_FILE_NAME);
        if manifest.exists() {
            return Some(manifest);
        }
    }
    None
}

// Returns first ancestor [`super::Manifest`] path for a given existing manifest
pub(crate) fn find_root_iter<'a>(manifest_path: &'a Path) -> impl Iterator<Item = PathBuf> + 'a {
    manifest_path
        .ancestors()
        .skip(2) // skip `manifest_path` and the parent dir
        .map(|dir| dir.join(MANIFEST_FILE_NAME))
        .filter(|path| path.exists())
}

// copied from:
// https://github.com/rust-lang/cargo/blob/a595d0da21f228b7fdae64d3d5c0e527ea66bb59/crates/cargo-util/src/paths.rs#L84-L84
/// Normalize a path, removing things like `.` and `..`.
///
/// CAUTION: This does not resolve symlinks (unlike
/// [`std::fs::canonicalize`]). This may cause incorrect or surprising
/// behavior at times. This should be used carefully. Unfortunately,
/// [`std::fs::canonicalize`] can be hard to use correctly, since it can often
/// fail, or on Windows returns annoying device paths. This is a problem Cargo
/// needs to improve on.
pub(crate) fn normalize_path(path: &Path) -> PathBuf {
    let mut components = path.components().peekable();
    let mut ret = if let Some(c @ Component::Prefix(..)) = components.peek().cloned() {
        components.next();
        PathBuf::from(c.as_os_str())
    } else {
        PathBuf::new()
    };

    for component in components {
        match component {
            Component::Prefix(..) => unreachable!(),
            Component::RootDir => {
                ret.push(Component::RootDir);
            }
            Component::CurDir => {}
            Component::ParentDir => {
                if ret.ends_with(Component::ParentDir) {
                    ret.push(Component::ParentDir);
                } else {
                    let popped = ret.pop();
                    if !popped && !ret.has_root() {
                        ret.push(Component::ParentDir);
                    }
                }
            }
            Component::Normal(c) => {
                ret.push(c);
            }
        }
    }
    ret
}

// Check if path holds at least one `.wit` file.
fn dir_contains_wit(dir: &Path) -> bool {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return false;
    };
    entries.filter_map(Result::ok).any(|entry| {
        entry.path().extension().is_some_and(|ext| ext == "wit")
            && entry.file_type().is_ok_and(|ft| ft.is_file())
    })
}

// Expand a string if is determined to be a glob pattern
pub(crate) fn expand_globs(entry: &Path, root_dir: &Path) -> Result<Vec<PathBuf>> {
    let joined = root_dir.join(entry);
    // TODO(mkatychev): handle escaping glob chars
    let is_glob = entry.to_str().is_some_and(|s| s.contains(['*', '?', '[']));
    let pattern = joined.to_str().filter(|_| is_glob);
    let Some(pattern) = pattern else {
        return Ok(vec![joined]);
    };
    let matches = glob::glob(pattern)?;
    let mut expanded: Vec<PathBuf> = matches
        .filter_map(Result::ok)
        .filter(|p| p.is_dir() && dir_contains_wit(p))
        .collect();

    if expanded.is_empty() {
        return Ok(vec![joined]);
    }

    expanded.sort();
    Ok(expanded)
}
