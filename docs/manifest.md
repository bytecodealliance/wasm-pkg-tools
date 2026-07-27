# Manifest file (`wkg.toml`)

Part of [wasm-pkg-tools docs](../README.md#documentation).

## `wkg.toml`

The `wkg.toml` manifest file is used to configure various parts of the tooling
and *is entirely optional*. Projects are not required to use this file.
Currently it serves two purposes: adding additional metadata and overriding
versions/dependencies.

### Format

```toml
[overrides]
"my:local-dep" = { path = "../local-dep/wit" }

[metadata]
authors = ["WasmPkg <wasm-pkg@bytecodealliance.org>"]
categories = ["wasm-pkg"]
description = "WASI HTTP interface"
license = "Apache-2.0"
documentation = "https://docs.foobar.baz"
homepage = "https://foobar.baz"
repository = "https://github.com/bytecodealliance/wasm-pkg-tools"
```

### `overrides`

- Type: table of `{ path = "<dir>" }`

Redirect a package reference to a local directory instead of fetching it from a
registry. The most common use is pointing at a sibling `wit/` folder while
developing two components together.

```toml
[overrides]
"my:local-dep" = { path = "../local-dep/wit" }
```

### `metadata.authors`

- Type: list of strings

Author list. Also consumed by downstream language tooling that reads
`wkg.toml`.

### `metadata.categories`

- Type: list of strings

Freeform category tags.

### `metadata.description`

- Type: string

Short description of the package. Emitted as the
`org.opencontainers.image.description` annotation on publish.

### `metadata.license`

- Type: string (SPDX expression)

License identifier. Emitted as `org.opencontainers.image.licenses`.

### `metadata.documentation`

- Type: string (URL)

Documentation URL. Currently informational.

### `metadata.homepage`

- Type: string (URL)

Project homepage. Emitted as `org.opencontainers.image.url`.

### `metadata.repository`

- Type: string (URL)

Source repository. Emitted as `org.opencontainers.image.source`.

## OCI annotation mapping

When publishing to OCI via `wkg publish`, `wkg` loads the metadata from the
wasm binary (which is automatically added to the WIT package with
`wkg wit build` if the metadata is present in the `wkg.toml` file). The
metadata is mapped to the following OCI annotations:

| `wkg.toml` metadata field | OCI annotation                         |
| ------------------------- | -------------------------------------- |
| `description`             | `org.opencontainers.image.description` |
| `license`                 | `org.opencontainers.image.licenses`    |
| `homepage`                | `org.opencontainers.image.url`         |
| `repository`              | `org.opencontainers.image.source`      |

Additionally, the `org.opencontainers.image.version` annotation is set to the
version of the package being published.

## Lockfile (`wkg.lock`)

Whenever `wkg` is used to fetch dependencies or build a wit package, it will
automatically create a `wkg.lock` file. This lock file is the other
standardized file that can be used by any other tooling integrating with
package tooling. Because components are cross-language, this file will be the
same for all languages. Here is an example v1 lockfile:

```toml
version = 1

[[packages]]
name = "wasi:cli"
registry = "wasi.dev"

[[packages.versions]]
requirement = "=0.2.0"
version = "0.2.0"
digest = "sha256:e7e85458e11caf76554b724ebf4f113259decf0f3b1ee2e2930de096f72114a7"

[[packages]]
name = "wasi:clocks"
registry = "wasi.dev"

[[packages.versions]]
requirement = "=0.2.0"
version = "0.2.0"
digest = "sha256:51911098e929732f65d1d84f8dc393299f18a9e8de632d854714f37142efe97b"

[[packages]]
name = "wasi:io"
registry = "wasi.dev"

[[packages.versions]]
requirement = "=0.2.0"
version = "0.2.0"
digest = "sha256:c33b1dbf050f64229ff4decbf9a3d3420e0643a86f5f0cea29f81054820020a6"

[[packages]]
name = "wasi:random"
registry = "wasi.dev"

[[packages.versions]]
requirement = "=0.2.0"
version = "0.2.0"
digest = "sha256:5d535edc544d06719cf337861b7917c3d565360295e5dc424046dceddb0a0e42"
```
