# Container assets

This directory stores OCI build definitions and integration scripts for
analysis tools executed by the Podman-backed DAG runtime.

## Layout

```text
containers/
  <tool>/
    Dockerfile
    test_<tool>_<analysis>.sh
```

An image contains the tool and its runtime dependencies only. Reference panels,
user data, credentials, and cluster configuration belong in the data catalog
or runtime configuration, not in an image.

For the end-to-end migration workflow, see
[Container Node Migration Workflow](../docs/container-node-migration.md).
For the published GHCR image inventory and runtime override, see
[GHCR container migration](../docs/ghcr-migration.md).
