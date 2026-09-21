# GHCR container migration

Runtime container references now default to:

```text
ghcr.io/auto-nomics/autonomics/<repository>@<digest>
```

The immutable inventory is stored in `containers/image-inventory.tsv`. Copies
are performed with `crane copy`, which preserves the manifest digest rather
than rebuilding and republishing a different image.

## Run the migration

Install crane, authenticate with a token that has `write:packages`, and run:

```sh
go install github.com/google/go-containerregistry/cmd/crane@v0.20.3
gh auth token | crane auth login ghcr.io -u "$GITHUB_USER" --password-stdin
containers/migrate-to-ghcr.sh
```

Use `--dry-run` to inspect the planned references, or `--only <repository>` for
a single row. `SOURCE_PREFIX` and `DESTINATION_PREFIX` can override both
namespaces. The manual GitHub Actions workflow
`.github/workflows/ghcr-migration.yml` runs the same process with
`GITHUB_TOKEN`.

New personal-account GHCR packages are private by default. GitHub requires the
package settings page for the initial public-visibility change; the REST package
API does not expose that operation. After the first version is public, later
pushes to the same package remain public.

## Runtime override

`AUTONOMICS_IMAGE_PREFIX` accepts a complete registry namespace such as
`ghcr.io/auto-nomics/autonomics` or `localhost/autonomics`. The production
default is `ghcr.io/auto-nomics/autonomics` when the variable is unset. The
legacy `ACR_ENDPOINT` variable is no longer read; setting it has no effect, and
all runtime pulls resolve through GHCR unless `AUTONOMICS_IMAGE_PREFIX`
explicitly overrides the namespace.
