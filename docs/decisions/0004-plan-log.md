# 0004 Plan log

Date: 2026-10-29.

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

Case against:

- The author runs the log and holds its key. RFC 9162 says a log can show different readers different views, and only readers who compare signed heads catch it (Sources).
- No one away from the box can check the log yet. The only copy of a head outside the plan log is the head line of a stamp file, kept in the study's `stamps/` folder; the time-stamp authority sees only the file's hash (0033). The check away from the box reads a study's records, not the plan log (0033). No verb publishes the plan-log head (0011, Not in this release).
- The family id is written into every plan leaf and never read, so the freeze rule has no code. It hashes `pins.payload`, which both templates leave empty. Plans with an empty pin, the same outcome ids and units, and the same hypothesis ids then share one family.
- A declared family with its own leaf now exists as well (0023), so "family" names two things in the log.
- The inclusion and consistency proofs (`planlog/merkle.rs`) are called only by tests. `verify` recomputes each root from all the leaves. A signed hash chain, the form the records use, allows the same check.

(written 2026-11-19)

What failure this prevents: a plan written after its data and passed off as written before (Why); a log rewritten to drop a plan that failed; and a reader who learns a private plan from its commitment. Expected, not observed in a study. (written 2026-11-19)

Constants:

- salt = 32 random bytes per plan. Basis: it keeps a private plan private (Why); for 32 bytes, none on file. Rule for change: none stated.
- family id = the first 16 hex characters of the SHA-256. Basis: none on file. Rule for change: none stated.

(written 2026-11-19)

Sources: https://www.rfc-editor.org/rfc/rfc9162
