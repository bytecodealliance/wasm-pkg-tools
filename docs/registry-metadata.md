# Registry metadata

Part of [wasm-pkg-tools docs](../README.md#documentation).

## Metadata via well-known URI (`/.well-known`)

For well-used or public registries, we recommend creating a
[`.well-known` metadata file](https://en.wikipedia.org/wiki/Well-known_URI)
that is used by the tool chain to simplify configuration and indicate to a
client which protocols and mappings to use (although this can be set directly
in [config](./configuration.md) as well).

The `wkg` tool and libraries expect a `registry.json` file to be present at a
specific location to indicate to the tooling where the components are stored.
For example, given a registry `example.com`, then the tooling will attempt to
find a `registry.json` file at
`https://example.com/.well-known/wasm-pkg/registry.json`.

## Format

```json
{
  "preferredProtocol": "oci",
  "oci": { "registry": "ghcr.io", "namespacePrefix": "webassembly/" }
}
```



<details>
  <summary>Deprecated variant</summary>
For backwards compatibility with previous tooling and versions of the `wkg`
tool, you may also encounter a `registry.json` file that looks different.
These files are still supported, but should be considered deprecated. For OCI
registries, the JSON looks like this:

```json
{
  "ociRegistry": "ghcr.io",
  "ociNamespacePrefix": "webassembly/"
}
```
</details>

---

### `preferredProtocol`

- Type: string
- Default: none (inferred if only one protocol block is present)

Which protocol the client should use when contacting the registry. While this
field is present for future compatibility, it is generally fixed to `"oci"` in
this implementation.

### `oci.registry`

- Type: string (host)

Base URL of the OCI registry that stores the components.

### `oci.namespacePrefix`

- Type: string
- Default: `""`

Prefix joined to the package's namespace when composing the OCI reference. For
the example above (which is for `wasi.dev`), components are available at
`ghcr.io/webassembly/$NAMESPACE/$PACKAGE:$VERSION` e.g. `ghcr.io/webassembly/wasi/http:0.2.1`).


## Conventions for storing components in OCI

Astute observers will note that OCI requires a specific structure for how those
components are stored. To be clear, this does not apply to deployable artifacts
(such as those used by various runtimes), but only to WIT components or library
components. Based on the information in the `registry.json` file, the base URL
and namespace prefix will be joined together with the namespace and package
name to form the full URL. So if you have a custom company namespace called
`acme`, then a package called `acme:foo` should be stored with the name
`acme/foo`. If we use the `registry.json` file from the example above, then the
component will be stored at `ghcr.io/webassembly/acme/foo:0.1.0`.

The tag _MUST_ be a valid semantic version or the tooling will ignore it when
pulling.
