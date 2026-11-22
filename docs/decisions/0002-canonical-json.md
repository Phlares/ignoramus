# 0002 Canonical JSON

Date: 2026-10-29.

Amended: 2026-11-21. Datetimes keep the text that `toml` 0.9 printed, as the loader writes them since #44; the text held the earlier line until now (#129).

A study spec is one TOML file. We hash its parsed values, not its bytes.

Decision:

- The spec parses to a `serde_json::Value`. We serialize it as compact JSON with object keys sorted, then hash with SHA-256.
- Keys sort because `serde_json::Map` is a `BTreeMap`. The `preserve_order` feature must stay off.
- The loader removes a leading UTF-8 byte order mark and turns CRLF into LF before parsing. A file saved on Windows hashes the same as its LF twin.
- Integer and float stay distinct in the hash: `1` and `1.0` differ, as do `0.0` and `-0.0`. TOML v1.0.0 types them apart.
- TOML allows `nan` and `inf`. JSON does not. The loader writes them as the strings `!nonfinite:nan`, `!nonfinite:inf` and `!nonfinite:-inf`, so loading never fails. The `finite` check then reports `E-NUM-001`.
- Datetimes hash as the text that `toml` 0.9 printed: seconds always, no zero fraction. `07:32` and `07:32:00.000` both hash as `07:32:00` (#44).

Why:

- People edit specs on Windows. A hash that changes with line endings would flag a plan as edited when it is not.
- Sorted compact output is one fixed form. It needs no custom serializer.

Cost: the form is not RFC 8785. Another tool must use the same rules to reproduce a hash.

Case against:

- A hash of the text would meet the reason on file (Why). The loader already strips the byte order mark and CRLF (Decision) and keeps that text (`spec/mod.rs`). A SHA-256 of that text does not change with line endings, and a reader can compute it with common tools once the byte order mark and CRLF are removed. The hash of parsed values also ignores key order, spacing and comments. The record gives no reason for that.
- A hash of parsed values follows the parser's version; a hash of the text does not. The move from `toml` 0.9 to 1 changed how two datetime forms print, and both changes reached the hash. #44 undid them in the loader and added a compatibility test.
- RFC 8785 is a published form; this one is the project's own (Cost). The record does not state all of its rules: number text and string escapes are whatever `serde_json` writes (`canon.rs`).

(written 2026-11-21)

What failure this prevents: a plan flagged as edited when only its line endings (Why), its byte order mark or its key order (Decision) changed; and two plans with different values that share one hash, such as `1` and `1.0` (Decision). Expected, not observed in a study. (written 2026-11-21)

Constants: none. This record fixes a form, not a tuned value. SHA-256, the key order and the three `!nonfinite:` strings are parts of that form. (written 2026-11-21)

Sources:

- RFC 8259, the JSON format: https://www.rfc-editor.org/rfc/rfc8259
- TOML v1.0.0: https://toml.io/en/v1.0.0
