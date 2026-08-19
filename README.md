# wasm-pkg-tools

<div align="center">
  <strong>A <a href="https://bytecodealliance.org/">Bytecode Alliance</a> project</strong>
</div>

Tools to package up [Wasm Components](https://github.com/webassembly/component-model)

This repo contains several Rust crates that can be used for fetching and publishing Wasm Components
to OCI registries. It is also the home of the `wkg` command line tool, which exposes all the
functionality of the libraries. The first (but not only) focus of this project is allow for fetching
of Wit Interfaces stored as components for use in creating components. It can also be used to fetch
and publish component "libraries" to/from a registry.

## Documentation

- [Install](#install)
- [Quick start](#quick-start)
- [Configuration (`config.toml`)](docs/configuration.md)
  - [Default fallback registries](docs/configuration.md#default-fallback-registries)
- [Registry metadata (`/.well-known`)](docs/registry-metadata.md)
  - [Conventions for storing components in OCI](docs/registry-metadata.md#conventions-for-storing-components-in-oci)
- [Manifest (`wkg.toml`)](docs/manifest.md)
  - [OCI annotation mapping](docs/manifest.md#oci-annotation-mapping)
- [Contributing](#contributing)
- [License](#license)

## Install

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

## Quick start

Set the default registry:

```sh
wkg config --default-registry {REGISTRY_DOMAIN}
```

Open the full config file in `$EDITOR`:

```sh
wkg config --edit
```

Download a package:

```sh
wkg get wasi:http@0.2.1
```

Publish a package to the configured registry:

```sh
wkg publish path/to/component.wasm
```

Pull a component directly from an OCI registry by [tag or by digest](https://specs.opencontainers.org/image-spec/annotations/?v=v1.1.1#IMAGE-SPEC-ANNOTATIONS-4:~:text=SPDX%20License%20Expression%2E-,org%2Eopencontainers%2Eimage%2Eref%2Ename,-Name%20of%20the) to pin an exact immutable artifact:

```sh
wkg oci pull ghcr.io/webassembly/wasi/http:0.2.1
# pulls the equivalent artifact
wkg oci pull ghcr.io/webassembly/wasi/http@sha256:49be4c9201a1b9fe99d589cde205d6096384a77b8767fedeef0e53e480abff49
```

Fetch WIT dependencies for the current project (uses `wkg.toml` / `wkg.lock`):

```sh
wkg fetch
```

Update pinned versions in `wkg.lock`:

```sh
wkg update
```

Build a WIT package into a component:

```sh
wkg build
```

Point at a non-default config or cache directory (the `--config` / `--cache` flags attach to each
subcommand, not to `wkg` itself):

```sh
wkg get --config .wkg/config.toml --cache ./wkg-cache wasi:cli@0.2.0
```

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
