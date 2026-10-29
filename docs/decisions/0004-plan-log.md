# 0004 Plan log

`register` records each approved plan in an append-only log. Anyone can later check that the log only grew.

Decision:

- The log is a Merkle tree as in RFC 9162 section 2.1. A leaf hashes as SHA-256 of `0x00` and the leaf bytes. A node hashes as SHA-256 of `0x01` and its two children.
- A leaf holds a commitment: SHA-256 of a random 32-byte salt and the canonical spec. The commitment binds the plan without showing it.
- A leaf also holds a family id: the first 16 hex characters of SHA-256 over the payload pin, the outcome ids and units, and the hypothesis ids. Plans that test the same thing share a family.
- Each append writes a tree head: size, root and time, signed with the local ed25519 key.
- One exclusive file lock covers the whole append: leaf, root and head. Two terminals both land, and the head verifies.
- Leaves after the last signed head are legal. They mean a crash between the two writes. The next append signs them.
- `verify` recomputes each head's root from the leaves and checks each signature.

Freeze rule:

- The plan log stays private until a claim from its family publishes. Then the whole family opens, so a reader can see every plan that was tried, not only the one that worked.

Why:

- A signed, append-only log shows that a plan existed before its data did.
- The salt keeps a private plan private while its commitment sits in the log.

Source: https://www.rfc-editor.org/rfc/rfc9162
