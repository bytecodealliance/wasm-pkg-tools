# wasm-pkg-tools

<div align="center">
  <strong>A <a href="https://bytecodealliance.org/">Bytecode Alliance</a> project</strong>
</div>

Tools to package up [Wasm Components](https://github.com/webassembly/component-model)

This repo contains several Rust crates that can be used for fetching and publishing Wasm Components
to OCI or Warg registries. It is also the home of the `wkg` command line tool, which exposes all the
functionality of the libraries. The first (but not only) focus of this project is allow for fetching
of Wit Interfaces stored as components for use in creating components. It can also be used to fetch
and publish component "libraries" to/from a registry.

## Installation

Right now installation of `wkg` is manual. You can either clone the repo and build from source, or
download a pre-built binary from the [releases
page](https://github.com/bytecodealliance/wasm-pkg-tools/releases) and add it to your `PATH`. In the
future we will add this tool to various package managers. If this is something you'd like to help
with, please feel free to open some PRs!

If you have a Rust toolchain installed, you can also install `wkg` with one of the following
options:

```sh
cargo install wkg
```

If you have [cargo-binstall](https://github.com/cargo-bins/cargo-binstall) installed, you can also
install `wkg` with a pre-built binary:

```sh
cargo binstall wkg
```

## Configuration

To quickly set the default registry:
```
wkg config --default-registry {REGISTRY_DOMAIN}
```

For the complete configuration options, you can edit the config file with your default
editor (set with env var `$EDITOR`):
```
wkg config --edit
```

The `wkg` tool and libraries use a configuration file to store settings. This config file is still
subject to change but we will try to keep it backwards compatible as we continue to develop the
tool. This config file is meant to be used by both `wkg` and also any other language-specific
component tooling that wants to fetch from registries. This should allow for a single configuration
file that can be used by all tooling, whether that be `wkg` or some other tool that isn't written in
Rust.

The default location is `$XDG_CONFIG_HOME/wasm-pkg/config.toml` on unix-like systems and
`{FOLDERID_RoamingAppData}\wasm-pkg\config.toml` on Windows but this can be overridden with the
`--config` flag. Examples of this are found below:

| Platform | Path                                            |
| -------- | ----------------------------------------------- |
| Linux    | `/home/<username>/.config`                      |
| macOS    | `/Users/<username>/Library/Application Support` |
| Windows  | `C:\Users\<username>\AppData\Roaming`           |

The configuration file is TOML and can be edited manually.

Below is an annotated example of a configuration file that shows all the available options.

```toml
# The default registry to use when none is specified. Generally this is wasi.dev, but can be set
# for cases when a company wants to use a private/internal registry.
default_registry = "acme.registry.com"

# This section contains a mapping of namespace prefixes (i.e. the "wasi" part of "wasi:http") to
# registries. This is used to determine which registry to use when fetching or publishing a
# component. If a namespace is not listed here, the default registry will be used.
[namespace_registries]
wasi = "wasi.dev"
example = "example.com"
# An example of providing your own registry mapping. For large and/or public registries, we
# recommend creating a well-known metadata file that can be used to determine the registry to use
# (see the section on "metadata" below). But many times you might want to override mappings or
# provide something that is used by a single team. The registry name does not matter, but must be
# parsable to URL authority. This name is purely used for mapping to registry config and isn't
# actually used as a URL when metadata is provided 
another = { registry = "another", metadata = { preferredProtocol = "oci", "oci" = {registry = "ghcr.io", namespacePrefix = "webassembly/" } } }

# This overrides the default registry for a specific package. This is useful for cases where a 
# package is published to multiple registries. 
[package_registry_overrides]
"example:foo" = "example.com"
# Same as namespace_registries above, but for a specific package.
"example:bar" = { registry = "another", metadata = { preferredProtocol = "oci", "oci" = {registry = "ghcr.io", namespacePrefix = "webassembly/" } } }

# This section contains a mapping of registries to their configuration. There are currently 3
# supported types of registries: "oci", "warg", and "local". The "oci" type is the default. The
# example below shows a use case that isn't yet super common (registries that speak multiple protocols)
# but is included for completeness.
[registry."acme.registry.com"]
# This field is only required if more that one protocol is supported. It indicates which protocol
# to use by default. If this is not set, then the fallback (oci) will be used.
default = "warg"
[registry."acme.registry.com".warg]
# A path to a valid warg config file. If this is not set, the `wkg` CLI (but not the libraries) 
# will attempt to load the config from the default location(s).
config_file = "/a/path"
# An optional authentication token to use when authenticating with a registry.
auth_token = "an-auth-token"
# An optional key for signing the component. Ideally, you should just let warg use the keychain
# or programmatically set this key in the config without writing to disk. This offers an escape
# hatch for when you need to use a key that isn't in the keychain.
signing_key = "ecdsa-p256:2CV1EpLaSYEn4In4OAEDAj5O4Hzu8AFAxgHXuG310Ew="
[registry."acme.registry.com".oci]
# The auth field can either be a username/password pair, or a base64 encoded `username:password` 
# string. If no auth is set, the `wkg` CLI (but not the libraries) will also attempt to load the
# credentials from the docker config.json. This field is also optional and if not set, anonymous
# auth will be used. If you're just pulling from a public registry, this is likely not required.
# If you're using a private registry and/or publishing, you'll almost certainly need to set this.
auth = { username = "open", password = "sesame" }
# This is an optional field that tells the OCI client to use a specific http protocol. If this is
# not set or not one of the accepted values of "http" or "https", then the default (https) will
# be used.
protocol = "https"
[registry."acme.registry.com".local]
# This is a required field that specifies the root directory on a filesystem where the components
# are stored. This is mostly used for local development and testing.
root = "/a/path"

# If a registry only has a config section for one protocol, then that protocol is automatically
# the default. The following is equivalent to:
# [registry."example.com"]
# default = "warg"
# [registry."example.com".warg]
# config_file = "/a/path"
[registry."example.com".warg]
config_file = "/a/path"

# Configuration for the "another" registry defined above.
[registry."another".oci]
auth = { username = "open", password = "sesame" }
```

### Well-known metadata

For well-used or public registries, we recommend creating a well-known metadata file that is used by
the tool chain to simplify configuration and indicate to a client which protocols and mappings to
use (although this can be set directly in config as well). The `wkg` tool and libraries expect a
`registry.json` file to be present at a specific location to indicate to the tooling where the
components are stored. For example, if a registry was `example.com`, then the tooling will attempt
to find a `registry.json` file at `https://example.com/.well-known/wasm-pkg/registry.json`. 

A full example of what this `registry.json` file should look like is below:

```json
{
  "preferredProtocol":"warg",
  "warg": {"url":"https://warg.example.com"},
  "oci": {"registry": "ghcr.io", "namespacePrefix": "webassembly/"}
}
```

The `preferredProtocol` field is optional and specifies which protocol the registry expects you to
use in the case where it supports both OCI and Warg. If both `warg` and `oci` config is in the
`registry.json` it is _highly recommended_ that this field be set. 

For the `oci` config, the `registry` field is the base URL of the OCI registry, and the
`namespacePrefix` field is the prefix that is used to store components in the registry. So in the
example above (which is for wasi.dev), the components will be available at
`ghcr.io/webassembly/$NAMESPACE/$PACKAGE:$VERSION` (e.g. `ghcr.io/webassembly/wasi/http:0.2.1`).

For the `warg` config, the `url` field is the base URL of the Warg registry used when connecting the
client. Namespacing for warg is built in to the protocol.

Please note that for backwards compatibility, with previous tooling and versions of the `wkg` tool,
you may also encounter a `registry.json` file that looks different. These files are still supported,
but should be considered deprecated.

For OCI registries, the JSON looks like this:

```json
{
        "ociRegistry": "ghcr.io",
        "ociNamespacePrefix": "webassembly/"
}
```



For Warg registries, the JSON looks like this:

```json
{
  "wargUrl": "https://warg.wa.dev"
}
```

### Conventions for storing components in OCI

Astute observers will note that OCI requires a specific structure for how those components are
stored. To be clear, this does not apply to deployable artifacts (such as those used by various
runtimes), but only to WIT components or library components. Based on the information in the
`registry.json` file, the base URL and namespace prefix will be joined together with the namespace
and package name to form the full URL. So if you have a custom company namespace called `acme`. Then
a package called `acme:foo` should be stored with the name `acme/foo`. If we use the `registry.json`
file from the example above, then the component will be stored at
`ghcr.io/webassembly/acme/foo:0.1.0`. Please note that the tag _MUST_ be a valid semantic version or
the tooling will ignore it when pulling.

## Contributing
Want to join us? Check out our ["Contributing" guide][contributing] and take a look at some of these
issues:

- [Issues labeled "good first issue"][good-first-issue]
- [Issues labeled "help wanted"][help-wanted]

[contributing]: https://github.com/bytecodealliance/wasm-pkg-tools/blob/master.github/CONTRIBUTING.md
[good-first-issue]: https://github.com/bytecodealliance/wasm-pkg-tools/labels/good%20first%20issue
[help-wanted]: https://github.com/bytecodealliance/wasm-pkg-tools/labels/help%20wanted

## License

<sup> Licensed under <a href="LICENSE">Apache-2.0 WITH LLVM-exception</a> </sup>

<br/>

<sub> Unless you explicitly state otherwise, any contribution intentionally submitted for inclusion
in this crate by you, as defined in the Apache-2.0 license with LLVM-exception, shall be licensed as
above, without any additional terms or conditions. </sub>
