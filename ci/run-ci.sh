#!/usr/bin/env bash

set -ex

cargo clippy --workspace --all-features
cargo test --workspace --all-features
(cd crates/wasm-pkg-client/tests/e2e && cargo run)