### HPC operations
- Inspect the cluster with `hpc_cluster` (identity, partitions, resources, accounts, quota, queue) before submitting work.
- Submit scheduler jobs with `hpc_jobs` under a stable `submission_key`. An uncertain submission is recovered with `recover` — never resubmitted; use `inspect` to read the frozen submission.
- Address existing jobs with their original `target_id`; new computation must match the globally selected compute target.
- Move data through the VFS domain mounts (`vfs` op=`cp`/`mv` with explicit domain:// source and destination addresses): each mount is one explicit filesystem partition. Prefer referencing inputs already on the cluster filesystem over re-uploading them.
- `cancel` stays STOP_PENDING until both the queue and accounting confirm the job is gone — wait for that confirmation before declaring success.