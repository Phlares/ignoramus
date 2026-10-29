# 0011 Publish bundle

Two crates prepare a study for publication. `bundle` builds the files. `deposit` sends them to an archive. This release deposits only on the Zenodo sandbox.

Decision:

- `bundle` has no network and no clock. The caller passes facts, dates and versions. `deposit` reaches the network only through one `Transport` trait.
- Both crates use only dependencies already listed in `0001-dependencies.md`. `deposit` also needs the `curl` program on the path. https://curl.se/docs/manpage.html

Study folder:

- The top level of a study folder holds the files `README.md`, `citation.toml` and `spec.toml`, and the folders `LICENSES`, `data`, `doc`, `records`, `results` and `src`. `LICENSES/` and `results/` are required. The build refuses any other top-level name.
- Generated files: `ARTIFACTS.sha256`, `CITATION.cff`, `LICENSE.md`, `MANIFEST.sha256`, `datacite.json`, `publish-profile.json` and `ro-crate-metadata.json`. A file of the same name at the top of the study is replaced.
- The bundle reads every study file through one checked handle. The walk records each file's device and inode. A read refuses a file, or a parent folder, swapped for a link since the walk. Copied files are hashed again and compared with the walk. The SPDX header check reads the first 64 KiB that the hash read kept, and opens no file.
- A file name that a manifest or zip cannot hold is refused, never renamed. A problem path escapes control, C1 and bidi characters.
- The study id names the bundle zip, so it holds only letters, digits, dots, hyphens and underscores.
- The bundle folder must lie outside the study folder. The build refuses an out path inside it before it makes any folder.

Citation:

- `citation.toml` is the one authored source. It uses the Citation File Format's own words, including the reference types `article`, `data` and `software`. https://citation-file-format.github.io/
- Three copies are generated from it: `CITATION.cff`, `datacite.json` and the RO-Crate. Specs hold no personal data.
- The tool writes the CFF itself and adds no YAML parser. Text is quoted so values like `no`, `1.10` or `a: b` stay strings. `cffconvert` 2.0.0 validates the output against schema 1.2.0. The checks on `citation.toml` follow the same schema, including its DOI and url patterns.
- `datacite.json` follows DataCite metadata schema 4. https://schema.datacite.org/meta/kernel-4/

RO-Crate:

- The crate follows RO-Crate 1.3 and the Process Run Crate 0.6 profile. The profile is a contextual entity. https://www.researchobject.org/ro-crate/specification/1.3/ https://www.researchobject.org/workflow-run-crate/profiles/process_run_crate/
- Run ids are version 8 UUIDs (RFC 9562) derived from the study id and the run id. A bundle is byte-identical across builds. https://www.rfc-editor.org/rfc/rfc9562
- Data-entity ids are percent-encoded.

Manifest and zip:

- `MANIFEST.sha256` is in `sha256sum` format. `sha256sum -c MANIFEST.sha256` checks a bundle.
- The zip is stored, not compressed. It has no Zip64. A file of 4 GiB or more is refused with E-PUB-012 before any byte is read. Such a file goes beside the bundle as an artifact. https://pkware.cachefly.net/webdocs/casestudies/APPNOTE.TXT
- A bundle zip holds at most 65,534 files. A count of 0xFFFF in the end record tells a reader to look for a Zip64 record, so 65,535 files or more are refused with E-PUB-012.

Licences:

- The bundle uses the REUSE `LICENSES/` folder layout: one file per SPDX id. https://reuse.software/spec-3.3/
- `LICENSE.md` always names the code licence text and the data licence text, so the build always requires both.
- Inputs keep their own licences. An input with an unspecified licence, or one that needs a request form, is never bundled. Two input records for one file are refused.
- The CC0 statement covers only our rights in the bundle.

Steps by profile:

- `rules/publish.toml` lists the publication steps for each profile (exploratory, replay, confirmatory). Each step is required, optional, a choice, opt-in, listed or none. The build refuses a bundle that lacks the file a required step names.

Deposit:

- The adapter targets the Zenodo sandbox only. It refuses any URL outside `https://sandbox.zenodo.org/` before a request leaves, including bucket and file URLs the server supplies. https://developers.zenodo.org/
- Two tokens. The stage token creates, describes, uploads and verifies. The publish token only publishes. A staging mistake cannot publish.
- Verify lists the draft's files with the documented files endpoint, `GET /api/deposit/depositions/:id/files`. It compares the file names with ours, downloads each file's `links.download`, and compares its SHA-256 with ours. Publish comes after verify.
- `curl` runs with `--config -` only. The URL, the token and the body go on stdin, so none shows in the process list. Globbing is off. Redirects are followed only for file downloads, because curl drops a custom `Authorization` header when a redirect changes host. A status outside 100-599 is a transport error.
- curl aborts only a stalled transfer: one slower than 1 KiB/s for 2 minutes, or a connection not made in 30 seconds. A long upload that keeps moving is never cut.
- An ignored live test (`crates/deposit/tests/sandbox.rs`) runs the whole path. It reads the token files named by `IGNORARIUM_ZENODO_STAGE_TOKEN_FILE` and `IGNORARIUM_ZENODO_PUBLISH_TOKEN_FILE`, and never prints a token.

Not in this release:

- The CLI `publish` verb: records to runs, report attachments, the plan-log head.
- `docs/rules` pages for the E-PUB codes.
- The replay profile in `build`. The build picks the profile from the claim label, so only `profile::named("replay")` reaches the replay profile until the replay verb exists.
- Git tag and folder SWHID, Software Heritage, Internet Archive, the index.
- Deposits to the real archive.

Why: the bundle is a pure function of the study, so a reader can rebuild and compare it. The deposit is a thin adapter that cannot reach the real archive or publish by mistake.
