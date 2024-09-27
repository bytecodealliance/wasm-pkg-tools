use std::path::{Path, PathBuf};

use tempfile::TempDir;
use wasm_pkg_client::{
    caching::{CachingClient, FileCache},
    Client,
};

pub fn fixture_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
}

pub async fn get_client() -> anyhow::Result<(TempDir, CachingClient<FileCache>)> {
    let client = Client::with_global_defaults().await?;
    let cache_temp_dir = tempfile::tempdir()?;
    let cache = FileCache::new(cache_temp_dir.path()).await?;

    Ok((cache_temp_dir, CachingClient::new(Some(client), cache)))
}

/// Loads the fixture with the given name into a temporary directory. This will copy the fixture from the tests/fixtures directory into a temporary directory and return the tempdir containing that directory (and its path)
pub async fn load_fixture(fixture: &str) -> anyhow::Result<(TempDir, PathBuf)> {
    let temp_dir = tempfile::tempdir()?;
    let fixture_path = fixture_dir().join(fixture);
    // This will error if it doesn't exist, which is what we want
    tokio::fs::metadata(&fixture_path).await?;
    let copied_path = temp_dir.path().join(fixture_path.file_name().unwrap());
    copy_dir(&fixture_path, &copied_path).await?;
    Ok((temp_dir, copied_path))
}

async fn copy_dir(source: impl AsRef<Path>, destination: impl AsRef<Path>) -> anyhow::Result<()> {
    tokio::fs::create_dir_all(&destination).await?;
    let mut entries = tokio::fs::read_dir(source).await?;
    while let Some(entry) = entries.next_entry().await? {
        let filetype = entry.file_type().await?;
        if filetype.is_dir() {
            // Skip the deps directory in case it is there from debugging
            if entry.path().file_name().unwrap_or_default() == "deps" {
                continue;
            }
            Box::pin(copy_dir(
                entry.path(),
                destination.as_ref().join(entry.file_name()),
            ))
            .await?;
        } else {
            // Skip any .lock files in the fixture
            if entry.path().file_name().unwrap_or_default() == ".lock" {
                continue;
            }
            tokio::fs::copy(entry.path(), destination.as_ref().join(entry.file_name())).await?;
        }
    }
    Ok(())
}
