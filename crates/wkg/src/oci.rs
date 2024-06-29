use std::{borrow::Cow, ops::Deref, path::PathBuf};

use anyhow::Context;
use clap::{Args, Subcommand};
use docker_credential::DockerCredential;
use oci_distribution::{
    client::{ClientConfig, ClientProtocol},
    secrets::RegistryAuth,
    Reference,
};
use oci_wasm::{WasmClient, WasmConfig};

#[derive(Debug, Args)]
pub struct Auth {
    /// The username to use for authentication. If no credentials are provided, wkg will load them
    /// from a local docker config and credential store and default to anonymous if none are found.
    #[clap(
        id = "username",
        short = 'u',
        env = "WKG_OCI_USERNAME",
        requires = "password"
    )]
    pub username: Option<String>,
    /// The password to use for authentication. This is required if username is set
    #[clap(
        id = "password",
        short = 'p',
        env = "WKG_OCI_PASSWORD",
        requires = "username"
    )]
    pub password: Option<String>,
}

impl Auth {
    fn into_auth(self, reference: &Reference) -> anyhow::Result<RegistryAuth> {
        match (self.username, self.password) {
            (Some(username), Some(password)) => Ok(RegistryAuth::Basic(username, password)),
            (None, None) => {
                let server_url = Self::get_docker_config_auth_key(reference);
                match docker_credential::get_credential(server_url.deref()) {
                    Ok(DockerCredential::UsernamePassword(username, password)) => {
                        return Ok(RegistryAuth::Basic(username, password));
                    }
                    Ok(DockerCredential::IdentityToken(_)) => {
                        return Err(anyhow::anyhow!("identity tokens not supported"));
                    }
                    Err(err) => {
                        tracing::debug!(
                            "Failed to look up OCI credentials with key `{server_url}`: {err}"
                        );
                    }
                }
                Ok(RegistryAuth::Anonymous)
            }
            _ => Err(anyhow::anyhow!("Must provide both a username and password")),
        }
    }

    /// Translate the registry into a key for the auth lookup.
    fn get_docker_config_auth_key(reference: &Reference) -> Cow<str> {
        match reference.resolve_registry() {
            "index.docker.io" => Cow::Borrowed("https://index.docker.io/v1/"), // Default registry uses this key.
            other => Cow::Owned(format!("https://{other}")),
        }
    }
}

#[derive(Debug, Args)]
pub struct Common {
    /// A comma delimited list of allowed registries to use for http instead of https
    #[clap(
        long = "insecure",
        default_value = "",
        env = "WKG_OCI_INSECURE",
        value_delimiter = ','
    )]
    pub insecure: Vec<String>,
}

/// Commands for interacting with OCI registries
#[derive(Debug, Subcommand)]
pub enum OciCommands {
    /// Pull a component from an OCI registry and write it to a file.
    Pull(PullArgs),
    /// Push a component to an OCI registry.
    Push(PushArgs),
}

impl OciCommands {
    pub async fn run(self) -> anyhow::Result<()> {
        match self {
            OciCommands::Pull(args) => args.run().await,
            OciCommands::Push(args) => args.run().await,
        }
    }
}

#[derive(Debug, Args)]
pub struct PullArgs {
    #[clap(flatten)]
    pub auth: Auth,

    #[clap(flatten)]
    pub common: Common,

    /// The OCI reference to pull
    pub reference: Reference,

    /// The output path to write the file to
    #[clap(short = 'o', long = "output")]
    pub output: Option<PathBuf>,
}

#[derive(Debug, Args)]
pub struct PushArgs {
    #[clap(flatten)]
    pub auth: Auth,

    #[clap(flatten)]
    pub common: Common,

    /// An optional author to set for the pushed component
    #[clap(short = 'a', long = "author")]
    pub author: Option<String>,

    // TODO(thomastaylor312): Add support for custom annotations
    /// The OCI reference to push
    pub reference: Reference,

    /// The path to the file to push
    pub file: PathBuf,
}

impl PushArgs {
    pub async fn run(self) -> anyhow::Result<()> {
        let client = get_client(self.common);
        let (conf, layer) = WasmConfig::from_component(&self.file, self.author)
            .await
            .context("Unable to parse component")?;
        let auth = self.auth.into_auth(&self.reference)?;
        client
            .push(&self.reference, &auth, layer, conf, None)
            .await
            .context("Unable to push image")?;
        println!("Pushed {}", self.reference);
        Ok(())
    }
}

impl PullArgs {
    pub async fn run(self) -> anyhow::Result<()> {
        let client = get_client(self.common);
        let auth = self.auth.into_auth(&self.reference)?;
        let data = client
            .pull(&self.reference, &auth)
            .await
            .context("Unable to pull image")?;
        let output_path = match self.output {
            Some(output_file) => output_file,
            None => PathBuf::from(format!(
                "{}.wasm",
                self.reference.repository().replace('/', "_")
            )),
        };
        tokio::fs::write(
            &output_path,
            data.layers
                .into_iter()
                .next()
                .ok_or_else(|| anyhow::anyhow!("No layers found"))?
                .data,
        )
        .await
        .context("Unable to write file")?;
        println!(
            "Successfully wrote {} to {}",
            self.reference,
            output_path.display()
        );
        Ok(())
    }
}

fn get_client(common: Common) -> WasmClient {
    let client = oci_distribution::Client::new(ClientConfig {
        protocol: if common.insecure.is_empty() {
            ClientProtocol::Https
        } else {
            ClientProtocol::HttpsExcept(common.insecure)
        },
        ..Default::default()
    });

    WasmClient::new(client)
}

#[cfg(test)]
mod tests {
    use crate::oci::Auth;
    use base64::{engine::general_purpose, Engine};
    use oci_distribution::{secrets::RegistryAuth, Reference};
    use serde_json::json;
    use tempfile::tempdir;

    #[test]
    fn into_auth_should_read_docker_registry_credentials() {
        let reference: Reference = "dockeraccount/image".parse().unwrap();
        verify_docker_config_credentials(&reference, "https://index.docker.io/v1/");
    }

    #[test]
    fn into_auth_should_other_registry_credentials() {
        let reference: Reference = "example.com/dockeraccount/image".parse().unwrap();
        verify_docker_config_credentials(&reference, "https://example.com");
    }

    fn verify_docker_config_credentials(reference: &Reference, key: &str) {
        let auth = Auth {
            username: None,
            password: None,
        };
        let temp_docker_config = tempdir().unwrap();
        let docker_config = temp_docker_config.path().join("config.json");
        let username = "some_user".to_string();
        let password = "some_password".to_string();
        let encoded_auth =
            general_purpose::STANDARD_NO_PAD.encode(format!("{username}:{password}"));
        let auths = json!({
            "auths": {
                key: {
                    "auth": encoded_auth
                },
            }
        });
        std::fs::write(docker_config, auths.to_string()).unwrap();
        std::env::set_var("DOCKER_CONFIG", temp_docker_config.path().as_os_str());
        let auth = auth.into_auth(reference).unwrap();
        assert_eq!(RegistryAuth::Basic(username, password), auth);
    }
}
