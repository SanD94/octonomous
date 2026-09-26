# Generated REST client

`crates/octonomous-core/src/generated/mod.rs` is generated from the pinned
`docs/openapi-v2.0.18.json` document. Regenerate it from the repository root:

```sh
./scripts/generate-client.sh
```

The generated source is committed, and normal workspace builds do not compile
or run Progenitor. The isolated `tools/codegen` utility pins Progenitor 0.15.0
and performs the minimum OpenAPI 3.1-to-3.0 normalization needed by Progenitor:

- nullable `anyOf` pairs and numeric `exclusiveMinimum` are translated to their
  equivalent OpenAPI 3.0 forms;
- redundant nullability is removed from optional headers to avoid invalid
  nested optional parameters in Progenitor 0.15 output;
- unsupported `deepObject` location parameters use normal query serialization;
  the handwritten transport layer in M2 owns location wire policy;
- operation error bodies remain raw `serde_json::Value`, because Progenitor
  requires one error type per operation while the API returns several tagged
  error schemas. M2 will decode those `_tag` envelopes centrally.

## Untagged-union audit

The pinned spec contains 22 named `anyOf` schemas and no discriminator fields.
Twenty are still decidable from JSON shape or a required constant field (for
example `type`, `status`, or `mode`) and retain typed generated representations.

Two schemas are intrinsically ambiguous and are deliberately generated as
`serde_json::Value`:

| Schema | Ambiguity |
|---|---|
| `Form.Value` | Its unrestricted string branch overlaps the `Infinity`, `-Infinity`, and `NaN` string branch. |
| `Model.ReasoningField` | Its unrestricted string branch overlaps the `reasoning`, `reasoning_content`, and `reasoning_text` string branch. |

This replacement happens before generation in `tools/codegen`; generated files
must not be patched by hand.
