# Manifest file (`wkg.toml`)

Part of [wasm-pkg-tools docs](../README.md#documentation).

## `wkg.toml`

The `wkg.toml` manifest file is used to configure various parts of the tooling
and *is entirely optional*. Projects are not required to use this file.
Currently it serves two purposes: adding additional metadata and overriding
versions/dependencies.

*NOTE*: A top-level `[workspace]` table is mutually exclusive with top-level
`[overrides]` and `[metadata]`: a workspace root uses `[workspace.metadata]`
instead, and members manifests have their own `[overrides]` / `[metadata]`.

### Format

Single-package layout:

```toml
[overrides]
"my:local-dep" = { path = "../local-dep/wit" }

[metadata]
authors = "WasmPkg <wasm-pkg@bytecodealliance.org>"
description = "WASI HTTP interface"
license = "Apache-2.0"
homepage = "https://foobar.baz"
repository = "https://github.com/bytecodealliance/wasm-pkg-tools"
revision = "f00ba4"
```

Workspace root layout:

```toml
[workspace]
members = ["pkg-a", "pkg-b/wit", "examples/*/wit"]

[workspace.metadata]
authors = "WasmPkg <wasm-pkg@bytecodealliance.org>"
license = "Apache-2.0"
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

A bare package name applies to every version of that package the WIT names. To
scope an override to one version, suffix the key with that exact version. This
is what lets a world name more than one version of the same package, since each
version can then point somewhere different:

```toml
[overrides]
"my:local-dep@0.1.0" = { path = "../local-dep-0.1.0/wit" }
"my:local-dep@0.2.0" = { path = "../local-dep-0.2.0/wit" }
```

A package cannot have both a bare and a versioned key, since the bare key
already covers every version; such a manifest is rejected rather than resolved
in an unspecified order.

Note that the `version` field is a *registry* version requirement and is
ignored when `path` is set — use a versioned key to select which version an
override applies to.

### `workspace.members`

- Type: list of strings (paths; gitignore-style globs allowed)

Directories that make up the workspace. Each entry is either a literal path or
a glob (e.g. `"examples/*/wit"`); globs expand relative to the workspace root
and skip entries with no `.wit` files. Used by `wkg fetch --workspace` and
`wkg publish --workspace` to operate on every member in one invocation.
Members share the workspace-level `wkg/deps` and `wkg/config.toml` next to the
root manifest.

### `workspace.metadata`

- Type: same shape as [`metadata`](#metadataauthors) below

Workspace-wide package metadata applied to every member. Set here instead of
top-level `[metadata]` when `[workspace]` is present.

### Member declarations

- Type: `{ workspace = "path/to/root" }` (optional)

A member `wkg.toml` typically has no `[workspace]` table at all; the root is
discovered by walking ancestor directories for a `wkg.toml` whose
`workspace.members` matches this manifest's path.
<!-- TODO -->
<!-- Point at the root explicitly with `workspace = "/path/to/wkg.toml"` if it lives outside the ancestor chain. -->

### `metadata.authors`

- Type: string (also accepts the legacy key `author`)

Author line. Unlike `Cargo.toml`, this is a single string, not a list.

### `metadata.description`

- Type: string

Short description of the package. Emitted as the
`org.opencontainers.image.description` annotation on publish.

### `metadata.license`

- Type: string (SPDX expression; serialized as `licenses`)

License identifier. Accepts the singular `license` key as an alias. Emitted as
`org.opencontainers.image.licenses`.

### `metadata.homepage`

- Type: string (URL)

Project homepage. Emitted as `org.opencontainers.image.url`.

### `metadata.repository`

- Type: string (URL; serialized as `source`)

Source repository URL. Accepts `source` or `repository` on input. Emitted as
`org.opencontainers.image.source`.

### `metadata.revision`

- Type: string

Source-control revision (commit hash, tag, etc.) the package was built from.

## OCI annotation mapping

When publishing to OCI via `wkg publish`, `wkg` loads the metadata from the
wasm binary (which is automatically added to the WIT package with
`wkg build` if the metadata is present in the `wkg.toml` file). The
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
