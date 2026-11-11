### Drug & Bioactivity Data (ChEMBL)
- Use `chembl_search` to resolve compound or target names into stable ChEMBL IDs.
- Use `chembl_molecule_summary` and `chembl_target_summary` for compound properties and protein components (interactive lookup).
- For pipeline data — activities, molecules, mechanisms of action, drug indications — use the DAG nodes `source_chembl_activities`, `source_chembl_molecules`, `source_chembl_mechanisms`, and `source_chembl_indications` to obtain typed tables for SQL and analysis nodes.