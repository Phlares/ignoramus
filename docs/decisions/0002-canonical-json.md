# 0002 Canonical JSON

A study spec is one TOML file. We hash its parsed values, not its bytes.

Decision:

- The spec parses to a `serde_json::Value`. We serialize it as compact JSON with object keys sorted, then hash with SHA-256.
- Keys sort because `serde_json::Map` is a `BTreeMap`. The `preserve_order` feature must stay off.
- The loader removes a leading UTF-8 byte order mark and turns CRLF into LF before parsing. A file saved on Windows hashes the same as its LF twin.
- Integer and float stay distinct in the hash: `1` and `1.0` differ, as do `0.0` and `-0.0`. TOML v1.0.0 types them apart.
- TOML allows `nan` and `inf`. JSON does not. The loader writes them as the strings `!nonfinite:nan`, `!nonfinite:inf` and `!nonfinite:-inf`, so loading never fails. The `finite` check then reports `E-NUM-001`.
- Datetimes hash as the text the `toml` crate prints.

Why:

- Reviewers edit specs on Windows. A hash that changes with line endings would flag a plan as edited when it is not.
- Sorted compact output is one fixed form. It needs no custom serializer.

Cost: the form is not RFC 8785. Another tool must use the same rules to reproduce a hash.

Sources:

- RFC 8259, the JSON format: https://www.rfc-editor.org/rfc/rfc8259
- TOML v1.0.0: https://toml.io/en/v1.0.0
