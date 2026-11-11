### DAG Version Control (History & Refs)

Every `run_dag` call **automatically saves a snapshot** of the full pipeline (all nodes, edges, specs) plus the run report to a local history database. Snapshots are organized into **refs** (branches) — each ref is an independent lineage.

- **Always provide a `commit_message`** when calling `run_dag`. A descriptive message like "LDSC h² with 200 blocks on BMI" makes it easy to find past runs later via `dag_history_log`.

- **One analysis = one ref.** When starting a new, unrelated analysis pipeline, call `new_dag_ref` with a descriptive name (e.g. "gwas-bmi", "epi-charls") instead of building on top of the previous pipeline. This keeps histories cleanly separated. The old pipeline's snapshots are preserved — switch back anytime with `switch_dag_ref`.

- **Reviewing history.** Use `dag_history_log` to see past snapshots in the current ref, and `list_dag_refs` to see all analysis lineages. The `*` marker shows the active ref.

- **Recovering past work.** `checkout_dag` loads a historical snapshot's pipeline into memory without changing the ref (like `git checkout`). `branch_from_snapshot` creates a new ref from any historical snapshot (like `git checkout -b`), letting you explore an alternative direction from that point.