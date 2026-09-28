# GHCR container images

Runtime container references resolve to:

```text
ghcr.io/auto-nomics/autonomics/<repository>@<digest>
```

Every published tool image is public and pinned by immutable manifest digest.
`containers/image-inventory.tsv` lists each repository together with its pinned
digest and published tag.

## Reference resolution

Image references are fully determined by their source data; there is no
deployment-time registry override. The legacy wrapper constants resolve
against the fixed `ghcr.io/auto-nomics/autonomics` namespace, and
manifest-level `[image]` blocks carry the registry host, namespace path, and
digest explicitly. To run against another registry (a local test registry, a
mirror), rewrite the address fields — a manifest digest is a content address
and survives the copy unchanged.

## Publishing an image

Build the image, push it to the GHCR namespace, then read the digest the
registry stored and pin it in the corresponding node module:

```sh
IMAGE=ghcr.io/auto-nomics/autonomics/<repository>:<tag>
podman build -f containers/<tool>/Dockerfile -t "$IMAGE" containers/<tool>
podman push "$IMAGE"
skopeo inspect "docker://$IMAGE" | jq -r .Digest
```

Update the node's `*_IMAGE_DIGEST` constant, the matching row in
`containers/image-inventory.tsv`, and the container README. New GHCR packages
are private by default; flip them to public in the package settings before
runtime pulls will work anonymously.
