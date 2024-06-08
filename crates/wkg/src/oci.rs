use std::path::PathBuf;

use anyhow::Context;
use clap::Args;
use oci_distribution::{
    client::{ClientConfig, ClientProtocol},
    secrets::RegistryAuth,
    Reference,
};
use oci_wasm::{WasmClient, WasmConfig};

#[derive(Debug, Args)]
pub struct Auth {
    /// The username to use for authentication
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

impl TryFrom<Auth> for RegistryAuth {
    type Error = anyhow::Error;
    fn try_from(auth: Auth) -> Result<Self, Self::Error> {
        match (auth.username, auth.password) {
            (Some(username), Some(password)) => Ok(RegistryAuth::Basic(username, password)),
            (None, None) => Ok(RegistryAuth::Anonymous),
            _ => Err(anyhow::anyhow!("Must provide both a username and password")),
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

#[derive(Debug, Args)]
pub struct GetArgs {
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
        let auth = self.auth.try_into()?;
        client
            .push(&self.reference, &auth, layer, conf, None)
            .await
            .context("Unable to push image")?;
        println!("Pushed {}", self.reference);
        Ok(())
    }
}

impl GetArgs {
    pub async fn run(self) -> anyhow::Result<()> {
        let client = get_client(self.common);
        let auth = self.auth.try_into()?;
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
