# Wasm Package Client

A minimal Package Registry interface for multiple registry backends.

## Running Tests

The e2e tests require an [OCI Distribution
Spec](https://github.com/opencontainers/distribution-spec)-compliant registry to
be running at `localhost:5000`. An ephemeral registry can be run with:

```console
$ docker run --rm -p 5000:5000 distribution/distribution:edge
```

The e2e tests themselves are in the separate [`tests/e2e`](./tests/e2e/) crate:

```console
$ cd tests/e2e
$ cargo run
```
