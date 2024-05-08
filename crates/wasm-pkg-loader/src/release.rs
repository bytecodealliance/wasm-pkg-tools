use std::path::Path;

use bytes::Bytes;
use futures_util::{future::ready, stream::once, Stream, StreamExt, TryStream, TryStreamExt};
use semver::Version;
use sha2::{Digest, Sha256};
use tokio::io::AsyncReadExt;

use crate::Error;

#[derive(Clone, Debug)]
pub struct Release {
    pub version: Version,
    pub content_digest: ContentDigest,
}

#[derive(Clone, Debug, PartialEq)]
pub enum ContentDigest {
    Sha256 { hex: String },
}

impl ContentDigest {
    pub async fn sha256_from_file(path: impl AsRef<Path>) -> Result<Self, std::io::Error> {
        let mut file = tokio::fs::File::open(path).await?;
        let mut hasher = Sha256::new();
        let mut buf = [0; 4096];
        loop {
            let n = file.read(&mut buf).await?;
            if n == 0 {
                break;
            }
            hasher.update(&buf[..n]);
        }
        Ok(hasher.into())
    }

    pub fn validating_stream(
        &self,
        stream: impl TryStream<Ok = Bytes, Error = Error>,
    ) -> impl Stream<Item = Result<Bytes, Error>> {
        let want = self.clone();
        stream.map_ok(Some).chain(once(async { Ok(None) })).scan(
            Sha256::new(),
            move |hasher, res| {
                ready(match res {
                    Ok(Some(bytes)) => {
                        hasher.update(&bytes);
                        Some(Ok(bytes))
                    }
                    Ok(None) => {
                        let got: Self = std::mem::take(hasher).into();
                        if got == want {
                            None
                        } else {
                            Some(Err(Error::InvalidContent(format!(
                                "expected digest {want}, got {got}"
                            ))))
                        }
                    }
                    Err(err) => Some(Err(err)),
                })
            },
        )
    }
}

impl std::fmt::Display for ContentDigest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ContentDigest::Sha256 { hex } => write!(f, "sha256:{hex}"),
        }
    }
}

impl From<Sha256> for ContentDigest {
    fn from(hasher: Sha256) -> Self {
        Self::Sha256 {
            hex: format!("{:x}", hasher.finalize()),
        }
    }
}

impl<'a> TryFrom<&'a str> for ContentDigest {
    type Error = Error;

    fn try_from(value: &'a str) -> Result<Self, Self::Error> {
        let Some(hex) = value.strip_prefix("sha256:") else {
            return Err(Error::InvalidContentDigest(
                "must start with 'sha256:'".into(),
            ));
        };
        let hex = hex.to_lowercase();
        if hex.len() != 64 {
            return Err(Error::InvalidContentDigest(format!(
                "must be 64 hex digits; got {} chars",
                hex.len()
            )));
        }
        if let Some(invalid) = hex.chars().find(|c| !c.is_ascii_hexdigit()) {
            return Err(Error::InvalidContentDigest(format!(
                "must be hex; got {invalid:?}"
            )));
        }
        Ok(Self::Sha256 { hex })
    }
}

impl std::str::FromStr for ContentDigest {
    type Err = Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        s.try_into()
    }
}

#[cfg(test)]
mod tests {
    use bytes::BytesMut;
    use futures_util::stream;

    use super::*;

    #[tokio::test]
    async fn test_validating_stream() {
        let input = b"input";
        let digest = ContentDigest::from(Sha256::new_with_prefix(input));
        let stream = stream::iter(input.chunks(2));
        let validating = digest.validating_stream(stream.map(|bytes| Ok(bytes.into())));
        assert_eq!(
            validating.try_collect::<BytesMut>().await.unwrap(),
            &input[..]
        );
    }

    #[tokio::test]
    async fn test_invalidating_stream() {
        let input = b"input";
        let digest = ContentDigest::Sha256 {
            hex: "doesn't match anything!".to_string(),
        };
        let stream = stream::iter(input.chunks(2));
        let validating = digest.validating_stream(stream.map(|bytes| Ok(bytes.into())));
        assert!(matches!(
            validating.try_collect::<BytesMut>().await,
            Err(Error::InvalidContent(_)),
        ));
    }
}
