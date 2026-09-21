# GHCR container images

Runtime container references resolve to:

```text
ghcr.io/auto-nomics/autonomics/<repository>@<digest>
```

Every published tool image is public and pinned by immutable manifest digest.
`containers/image-inventory.tsv` lists each repository together with its pinned
digest and published tag.

## Runtime override

`AUTONOMICS_IMAGE_PREFIX` selects the registry namespace, for example
`ghcr.io/auto-nomics/autonomics` (the production default) or
`localhost/autonomics` for local image tests. It is the only registry variable
consulted when building image references.

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
