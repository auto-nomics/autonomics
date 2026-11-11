### Central Resource Downloads
- Use `resource_download` for allowlisted HTTP(S) files and host-managed S3/OSS objects. It streams in the background and installs the final file atomically under `local://downloads`; poll `view_task_status` and collect the receipt with `wait_task` or `view_task_results`.
- For HTTP(S), provide `uri` plus `destination`. For S3/OSS, provide the host-configured `profile`, `bucket`, and `key`; never request or expose object-store credentials.
- Set `expected_sha256` when a checksum is available, and choose explicit filenames under `local://downloads`.