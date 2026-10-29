# 0007 Environment identity

A result is only comparable if the environment that made it can be named and rebuilt. The box names an environment four ways.

Decision:

- Recipe hash: SHA-256 over the Dockerfile and the `uv` lockfile. It names the inputs, not the built image.
- Image digest: the image id the container engine reports after a build. It names the built image.
- Stored artifact: the saved image goes into the content store under its SHA-256. The run can use the exact bytes even if the base image moves or vanishes.
- Rebuild rate: the fraction of fresh, no-cache builds that give the first build's image digest. The box measures it. It never assumes a recipe rebuilds the same.
- A Dockerfile whose base images are not pinned by `@sha256:` digest is flagged as unpinned. The flag warns. It does not block.

Status: the rebuild rate has not been measured on this host yet. Until it is, the stored artifact is the only identity the box claims.

Why: a pinned recipe can still rebuild to different bytes, for example through timestamps or package indexes. A measured rate says how far to trust a rebuild. The stored artifact does not depend on that rate.

Sources: https://docs.docker.com/reference/cli/docker/image/build/ and https://docs.astral.sh/uv/concepts/projects/layout/#the-lockfile
