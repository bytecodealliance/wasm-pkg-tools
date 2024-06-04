//! A crate containing configurations for use with config module of `wasm-pkg-common`.
use secrecy::{ExposeSecret, SecretString};
use serde::Serializer;

pub mod oci;
pub mod warg;

pub(crate) fn serialize_string_secret<S: Serializer>(
    secret: &SecretString,
    serializer: S,
) -> Result<S::Ok, S::Error> {
    serializer.serialize_str(secret.expose_secret())
}

pub(crate) fn serialize_option_string_secret<S: Serializer>(
    secret: &Option<SecretString>,
    serializer: S,
) -> Result<S::Ok, S::Error> {
    match secret {
        Some(sec) => serializer.serialize_str(sec.expose_secret()),
        None => serializer.serialize_none(),
    }
}

#[cfg(test)]
mod tests {
    use oci_distribution::client::ClientProtocol;
    use secrecy::ExposeSecret;

    use super::*;
    use wasm_pkg_common::{config::*, Registry};

    const TEST_CONFIG: &str = r#"
    default_registry = "example.com"

    [namespace_registries]
    wasi = "wasi.dev"

    [registry."wasi.dev"]
    type = "oci"
    oci = { auth = { username = "open", password = "sesame" }, protocol = "https" }

    [registry."example.com"]
    type = "warg"
    warg = { auth_token = "top_secret" }
    "#;

    #[test]
    fn parse_registry_configs() {
        let conf = Config::from_toml(TEST_CONFIG).expect("unable to parse config");
        let wasi_dev: Registry = "wasi.dev".parse().unwrap();
        let example_com: Registry = "example.com".parse().unwrap();

        let reg_conf = conf
            .registry_config(&wasi_dev)
            .expect("missing registry config");
        assert_eq!(
            reg_conf.backend_type().expect("missing backend type"),
            "oci",
            "should have the correct backend type"
        );
        let parsed: oci::OciRegistryConfig = reg_conf
            .backend_config("oci")
            .expect("unable to parse oci config")
            .expect("missing oci config");

        assert_eq!(
            parsed.protocol.expect("missing protocol"),
            ClientProtocol::Https,
            "should have the correct protocol"
        );
        let auth = parsed.auth.expect("missing auth");
        assert_eq!(auth.username, "open", "should have the correct username");
        assert_eq!(
            auth.password.expose_secret(),
            "sesame",
            "should have the correct password"
        );

        let reg_conf = conf
            .registry_config(&example_com)
            .expect("missing registry config");
        assert_eq!(
            reg_conf.backend_type().expect("missing backend type"),
            "warg",
            "should have the correct backend type"
        );
        let parsed: warg::WargRawConfig = reg_conf
            .backend_config("warg")
            .expect("unable to parse warg config")
            .expect("missing warg config");

        assert_eq!(
            parsed
                .auth_token
                .as_ref()
                .expect("Should have auth token set")
                .expose_secret(),
            "top_secret",
            "should have the correct auth token"
        );
    }
}
