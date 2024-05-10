#!/usr/bin/env bash

set -ex

cargo clippy --workspace
cargo test --workspace
(cd crates/wasm-pkg-loader/tests/e2e && cargo run)