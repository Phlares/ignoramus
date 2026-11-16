# 0033 Outside time stamps, and a check with the public key

Date: 2026-11-15. Decided and built.

A reader outside the box could not check a study. `verify` read the private key, so only the key holder could check the signatures (#85). A study could carry an RFC 3161 time stamp from an outside authority, but no record pointed at it, and the card printed "Outside time stamp: not in the records" (#79). Every time in the record log comes from the box's own clock.

Decision:

- **The file to stamp.** `stamp <id> --request [--with <file>]` verifies the study's log and writes these lines, with LF line ends:

  ```text
  register record hash of <id>: <hex>
  register record signature of <id>: <hex>
  box public key (ed25519): <hex>
  record head of <id>: seq <n>, hash <hex>
  record head signature of <id>: <hex>
  plan log head (<log>): <the last head of that plan log, as canonical JSON>
  ```

  The bytes of `--with` follow, for lines the box cannot know. The file goes to `<study>/stamps/<sha256 of the file>.txt`, write-once. The same log gives the same file. The head is the last record of the log when the file is written. The chain makes a head at seq n cover records 0 to n. The plan log head line comes from the verified plan log. It is left out when the plan log folder is missing or its log holds no signed head; when the folder exists and its log does not verify, `--request` fails. `--request` appends no record. The register line keeps one exact form, `register record hash of <id>: <hex>`, so a file made before this verb existed can still be recorded.
- **The commands.** `--request` prints the file's path and, for each authority URL in `<home>/config.toml` (`[stamp] authorities = [...]`), three commands: `openssl ts -query`, a `curl` call to the authority, and `ignorarium stamp <id> --reply`. With no authority set, the URL prints as a placeholder and the verb says so. The box makes no network call.
- **The reply.** `stamp <id> --reply <R> [--file <F>]` makes every check before its first write:
  - the reply is granted, its digest algorithm is SHA-256, and its digest is 32 bytes (`E-STP-001`);
  - the stamped file is `--file`, else `stamps/<digest>.txt`; it exists, and its SHA-256 is the reply's digest (`E-STP-002`);
  - the study's record log verifies; a log that does not fails with the verify error;
  - the file holds this study's register line byte for byte, no box line with another value than the box computes, no two different values for one line, and no head signature line without a head line (`E-STP-003`);
  - a head line names a record that the log holds with that hash (`E-STP-004`).

  It then copies the file and the reply into `stamps/`, write-once, and appends one signed `stamp` record. A reply already recorded appends nothing and prints "already recorded". Several replies are several records, in log order. The verb works before and after `done` (0034), with no terminal. It does not check the plan log head line of a file it is given.
- **Three cases per line.** For each box line, the file holds the exact line: `true`. The file holds no line with that line's prefix for this study: `false`. The file holds such a line with another value: refused, and nothing is written. So `true` never rests on a value the box did not compute.
- **The record.** A `stamp` record holds `file_sha256`, `reply_sha256`, `authority`, `stamped_at`, `serial`, `policy`, `lines` and `checked`.
  - `lines` says which box lines the file held. Its `head` is the seq and hash of the head record, or null.
  - `authority` is the reply's own authority field when that field is a directory name. Else it is the subject of the signer certificate inside the reply, else null. It is text from outside the box, so every terminal line and the card print it escaped.
  - `stamped_at` is the reply's time. `serial` is lowercase hex without the DER sign byte, as `openssl ts -reply -text` prints it.
  - `checked` reads "digest and lines; signature checked by the reader".
  - What a stamp covers is read from `lines`: the registration always, and the results when the head's seq is at or after the `done` record's seq.
- **A flag, not a refusal.** `verify` and the card flag a stamp whose `stamped_at` is earlier than the time of the record it covers: the head record, else the register record. The two times are compared to the second, because `stamped_at` holds whole seconds.
- **The card.** One "Outside time stamp" line per `stamp` record, in log order: the stamped time, the authority, what it covers, which key and signature lines the file held, and "signature checked by the reader". Then the check line once: `openssl ts -verify -data stamps/<file_sha256>.txt -in stamps/<reply_sha256>.tsr -CAfile <bundle>`. A study with no stamp prints "not in the records". The register record's `timestamp: "pending"` stays; the card does not read it.
- **The public key.** `key export` prints the box's ed25519 public key as lowercase hex and a newline: the text of the key line above. A box with no key refuses.
- **A check away from the box.** `verify --public-key <file> --records <file>` checks a copy of a study's log with that key. The two flags come together and exclude `--study`. This mode opens no box home and writes nothing. One function checks the log in both modes, so both print the same study lines: the record count, the register record hash, the registered box, the sessions and binary changes, the `done` line or "no done record", one line per `stamp` record with the seq and hash of its head, and the flags. Both modes fail on a `stamp` record that does not parse, and on a head that names no earlier record of the log with that hash. Both escape every line they print, because in the public-key mode the signer under check chose every string. Neither mode reads a stamp file. The public-key mode reads no plan log.
- **The parser.** `x509-tsp` reads the reply (0001). The box parses the reply and does not verify the authority's signature.

Why:

- A log's own clock is not checkable from outside. A time-stamp authority signs the time instead (sigstore).
- A stamp should cover the signature, not only the artifact (sigstore). So the file holds the signature lines beside the hashes.
- A time stamp is checkable only when anyone can recompute the hash from the archived bytes (dhimmel). So the box keeps the stamped file and the reply in `stamps/`, and the record names both by hash.
- The box writes the file from its verified records. A person or a script that calls the verb cannot mistype a hash. A file made by hand still records when it holds the register line, as the first confirmatory study's file does.
- The reader checks the authority's signature with the authority's certificate chain (sigstore). The box checks what it can compute: the digest and the lines. It keeps no trust store and needs no network.
- A stamp of the head after `done` fixes the result records in time. After that, the key holder cannot rewrite the chain without the change showing: the stamped head hash no longer matches. `verify` checks the head hash against the log and prints it, so a reader compares it with the stamped file.
- With `key export` and `verify --public-key`, a reader with the log, the key and the stamp files checks a study without the box's private key.

Case against:

- The box records a reply whose signature it never checked. A reply signed by a self-made certificate records as well as a real one. A reader who skips `openssl ts -verify` has only the box's word. We rejected a check in the box: it needs certificate chains, trust anchors and more crates.
- The box does not call the authority. A person or a script must run three printed commands and pass the reply back. We rejected an HTTP client in the box for now: a box may have no network, and #79 leaves the call for later. A later call replaces the printed commands and nothing else.
- We rejected a box that writes the request itself and accepts only its own file. It needs DER writing, and it would refuse the first confirmatory study's hand-made file, which holds only the register line.
- `authority` read from the signer certificate is text the authority chose. The box checks none of it.

Limits:

- One authority is one point of trust. A study may hold several stamps; the box checks none of their signatures.
- A stamp proves that the bytes existed by a date. It does not prove that no other version was also stamped, or who wrote the bytes (dhimmel). The plan log and its head line carry the single version.
- A file made by hand, such as the first confirmatory study's, holds no key line, no signature line and no head line. A record of such a file says so in `lines`: `key` is false and `head` is null.
- `opened` records written after a head stamp sit outside that stamp. They hold no result.
- The parser refuses two kinds of valid reply: one whose time has fractional seconds, and one that holds the same certificate twice. Both limits come from the `der` crate that `x509-tsp` uses, and tests pin them. Each refusal carries the parser's message; neither is a crash. A signer named by key identifier gives no authority name.
- A compact export of the log is not built (#85).
- The TeX card prints no stamp line and no "Study ended" line; only `card.md` and `card.json` do.

What failure this prevents: a stamp no record points to. The first confirmatory study was stamped by an outside authority, and its card printed "not in the records" (observed, #79). Also a key holder who could rewrite the chain after registration with no outside check (expected, #85).

Constants: none. The record sets no value. SHA-256 is the one digest the box accepts because it is the box's one hash.

Sources:

- sigstore blog, on trusted time: a log's own clock is not externally verifiable; a time-stamp authority signs the time; stamp the signature, not only the artifact; verify with the authority's certificate chain. https://blog.sigstore.dev/trusted-time/
- dhimmel blog, on irreproducible time stamps: a time stamp is checkable only when anyone can recompute the hash from archived bytes; it proves content existed by a date, not that no other version was stamped, and not who wrote it. https://blog.dhimmel.com/irreproducible-timestamps/
