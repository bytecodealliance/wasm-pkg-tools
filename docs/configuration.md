<!-- TODO: investigate something akin to https://github.com/Crazytieguy/cargo-doc-md for docgen from a jsonschema -->
# Configuration file (`config.toml`)

Part of [wasm-pkg-tools docs](../README.md#documentation).

The `wkg` tool and libraries use a configuration file to store settings. This config file is still
subject to change but we will try to keep it backwards compatible as we continue to develop the
tool. This config file is meant to be used by both `wkg` and also any other language-specific
component tooling that wants to fetch from registries. This should allow for a single configuration
file that can be used by all tooling, whether that be `wkg` or some other tool that isn't written in
Rust.

The default location is `$XDG_CONFIG_HOME/wasm-pkg/config.toml` on unix-like systems and
`{FOLDERID_RoamingAppData}\wasm-pkg\config.toml` on Windows but this can be overridden with the
`--config` flag.

| Platform | Path                                  |
| -------- | ------------------------------------- |
| Linux    | `/home/<username>/.config`            |
| macOS    | `/home/<username>/.config`            |
| Windows  | `C:\Users\<username>\AppData\Roaming` |

The configuration file is TOML and can be edited manually.

## Format

Summary of configuration (see [Configuration keys](#configuration-keys) for details):

```toml
default_registry = "acme.registry.com"

[namespace_registries]
wasi = "wasi.dev"
example = "example.com"
another = { registry = "another", metadata = { preferredProtocol = "oci", "oci" = { registry = "ghcr.io", namespacePrefix = "webassembly/" } } }

[package_registry_overrides]
"example:foo" = "example.com"
"example:bar" = { registry = "another", metadata = { preferredProtocol = "oci", "oci" = { registry = "ghcr.io", namespacePrefix = "webassembly/" } } }

[registry."acme.registry.com".oci]
auth = { username = "open", password = "sesame" }
protocol = "https"

[registry."acme.registry.com".local]
root = "/a/path"

[registry."example.com".oci]
auth = { username = "open", password = "sesame" }

[registry."another".oci]
auth = { username = "open", password = "sesame" }
```

## Configuration keys

### `default_registry`

- Type: string (URL authority)
- Default: none

The registry to use when a package's namespace is not covered by
`namespace_registries`, `package_registry_overrides`, or the built-in
[fallbacks](#default-fallback-registries). Typically `wasi.dev`, or set to a
private/internal registry for company use.

```toml
default_registry = "acme.registry.com"
```

### `namespace_registries`

- Type: table of `{ string | inline-table }`

Maps a namespace prefix (the `wasi` in `wasi:http`) to a registry. If a
namespace is not listed here, the [default registry](#default_registry) is used.
Values are either a plain registry name or an inline table with an embedded
`metadata` block that supplies the same fields as a
[well-known `registry.json`](./registry-metadata.md): useful when a registry
does not serve one.

```toml
[namespace_registries]
wasi = "wasi.dev"
another = { registry = "another", metadata = { preferredProtocol = "oci", "oci" = { registry = "ghcr.io", namespacePrefix = "webassembly/" } } }
```

### `package_registry_overrides`

- Type: table of `{ string | inline-table }`

Same shape as [`namespace_registries`](#namespace_registries), but keyed by
fully qualified package (`"namespace:name"`). Wins over the namespace mapping
and the default registry. Useful when one package is published to a different
registry than the rest of its namespace.

```toml
[package_registry_overrides]
"example:foo" = "example.com"
```

### `registry.<name>`

Per-registry configuration is nested under `[registry."<name>"]`. The two
supported backends are `oci` and `local`. If a registry declares only one
backend, that backend is the default; otherwise, set `default` explicitly.

```toml
[registry."example.com"]
default = "oci"
```

### `registry.<name>.oci.auth`

- Type: `{ username, password }` inline table *or* base64-encoded `username:password` string
- Default: none (anonymous)

Credentials for the OCI backend. If unset, the `wkg` CLI (but not the
libraries) also checks the Docker `config.json`. Anonymous auth is fine for
public read-only access; private registries and publish flows almost always
need this set.

```toml
[registry."acme.registry.com".oci]
auth = { username = "open", password = "sesame" }
```

### `registry.<name>.oci.protocol`

- Type: string (`"http"` or `"https"`)
- Default: `"https"`

Forces the HTTP scheme for the OCI client. Any other value falls back to the
default. Set to `"http"` for local test registries.

```toml
[registry."acme.registry.com".oci]
protocol = "https"
```

### `registry.<name>.local.root`

- Type: string (filesystem path)
- Required when the `local` backend is configured

Root directory on disk where the local backend stores components. Intended for
local development and testing.

```toml
[registry."acme.registry.com".local]
root = "/a/path"
```

## Default fallback registries

If no configuration is found, the following mapping of namespace prefixes is used as a fallback:

```toml
wasi = "wasi.dev"
ba = "bytecodealliance.org"
```

The `wkg` tool will therefore fetch registry metadata from the respective
[well-known URIs](https://en.wikipedia.org/wiki/Well-known_URI):

```text
https://wasi.dev/.well-known/wasm-pkg/registry.json
https://bytecodealliance.org/.well-known/wasm-pkg/registry.json
```

Both registries store their packages as OCI artifacts in the
[GitHub Package Registry](https://docs.github.com/en/packages/working-with-a-github-packages-registry/working-with-the-container-registry).

See [Registry metadata](./registry-metadata.md) for the `registry.json` schema.
