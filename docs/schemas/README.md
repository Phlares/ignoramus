# Schemas of the `--json` outputs

Each `--json` output of ignorarium is one JSON object. It holds its schema number under `schema`; `report --json` holds it under `card_version`. The file `<output>.v<n>.schema.json` in this folder describes the output with number `n`, as JSON Schema draft 2020-12. The number rises when a key is removed or renamed, or when a value changes type; a new key keeps the number, so a reader must accept keys it does not know.
