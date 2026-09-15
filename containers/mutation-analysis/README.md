# Mutation analysis container

This image packages the Bioconductor official runtime with `maftools` for
mutation-analysis operations. Mutation and clinical tables remain outside the
image; the Rust wrapper stages them through the existing container contract.

Published immutable ACR image digest:

```text
sha256:6f33327237fb5b5cb01b65b194244a742d791a0466844a6f5a9e18af2f99158a
```

The wrapper combines this digest with
`$ACR_ENDPOINT/autonomics/mutation-analysis`.

## Operations

The entrypoint reads `MUTATION_CONFIG` and emits a TSV report to
`AUTONOMICS_OUTPUT0` and JSON provenance/detail data to
`AUTONOMICS_OUTPUT1`.

- `tmb`: per-sample mutation burden against `panel_size_mb`. When
  `tmb_groups` contains two labels found in the optional clinical table, the
  JSON detail also contains the Wilcoxon p-value.
- `summary`: counts and fractions by variant classification, variant type, and
  SNV class.
- `top_genes`: genes ordered by mutation count, altered-sample count, and gene
  identifier.
- `titv`: transition and transversion counts and ratios by sample.
- `mutex`: two-sided Fisher exact tests for co-occurrence versus mutual
  exclusivity between pairs of the top altered genes.

The optional deconstructSigs-style `signatures` operation is not implemented.
Signatures require an external signature catalog and a separate dependency
contract; adding one later should be an explicit schema and image change.

## Build and smoke test

```bash
podman build \
  -f containers/mutation-analysis/Dockerfile \
  -t localhost/atc/mutation-analysis:0.1.0 \
  containers/mutation-analysis/

containers/mutation-analysis/test_mutation.sh
```

Set `BUILD_IMAGE=0` to reuse the existing
`localhost/atc/mutation-analysis:0.1.0` image.

## Publish and verify

```bash
ACR_ENDPOINT=${ACR_ENDPOINT:-crpi-isjkczwpadlvr9i3.cn-hongkong.personal.cr.aliyuncs.com}
ACR_IMAGE="$ACR_ENDPOINT/autonomics/mutation-analysis:0.1.0"
podman tag localhost/atc/mutation-analysis:0.1.0 "$ACR_IMAGE"
podman push "$ACR_IMAGE"
# Aliyun can normalize the pushed manifest, so inspect the remote tag rather
# than relying on the pre-push local image digest.
REMOTE_DIGEST=$(skopeo inspect "docker://$ACR_IMAGE" | jq -r .Digest)
printf 'published digest: %s\n' "$REMOTE_DIGEST"
podman pull "$ACR_ENDPOINT/autonomics/mutation-analysis@$REMOTE_DIGEST"
```

Always publish the digest returned by the remote tag after registry
normalization. Rebuild, retag, republish, and update both
`MUTATION_ANALYSIS_IMAGE_DIGEST` and this document whenever the base image,
R package, or script changes.
