# 0007 Environment identity

Date: 2026-10-29.

A result is only comparable if the environment that made it can be named and rebuilt. The box names an environment four ways.

Decision:

- Recipe hash: SHA-256 over the Dockerfile and the `uv` lockfile. It names the inputs, not the built image.
- Image digest: the image id the container engine reports after a build. It names the built image.
- Stored artifact: the saved image goes into the content store under its SHA-256. The run can use the exact bytes even if the base image moves or vanishes.
- Rebuild rate: the fraction of fresh, no-cache builds that give the first build's image digest. The box measures it. It never assumes a recipe rebuilds the same.
- A Dockerfile whose base images are not pinned by `@sha256:` digest is flagged as unpinned. The flag warns. It does not block.

Status: the rebuild rate has not been measured on this host yet. Until it is, the stored artifact is the only identity the box claims.

Why: a pinned recipe can still rebuild to different bytes, for example through timestamps or package indexes. A measured rate says how far to trust a rebuild. The stored artifact does not depend on that rate.

Case against:

- Only the stored artifact reaches the signed records. The recipe hash, the image digest and the unpinned-base flag go only into the `env.build` event, at `warn` level for the flag (`runner.rs`; events and records, 0029). No card line or record reads that event.
- No command computes the rebuild rate (Status). Only tests call it, and its one test against a real engine is ignored.
- At level 2 the stored image is not all a row runs. The initramfs and the rootfs disk sit outside the content store (#32); the guest kernel has its own pin in code (0010). An image with a file its owner cannot read cannot become a disk, so such a study runs at level 1, or is refused at `min_isolation = 2` (#32, comment).

(written 2026-11-19)

What failure this prevents: a result whose environment cannot be named or rebuilt, and a claim that a recipe rebuilds the same bytes when no one measured it (Status). Expected, not observed in a study. (written 2026-11-19)

Constants: none. This record sets no fixed value; the rebuild rate is a measured result. (written 2026-11-19)

Sources: https://docs.docker.com/reference/cli/docker/image/build/ and https://docs.astral.sh/uv/concepts/projects/layout/#the-lockfile
