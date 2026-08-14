# DAG Reverse-Compilation Design

## Goal

Every DAG node that wraps a statistical method can be "reverse-compiled" into
the equivalent R / Python script that calls the original reference package.
The generated script is:

1. **Human-auditable** — a researcher can read it and verify the analysis.
2. **Independently runnable** — `Rscript output.R` reproduces the result.
3. **Differential-testable** — CI can run both Rust and R paths and diff outputs.

## Design principles

- **Factory-level, not node-level.** Codegen operates on `(kind, spec)` pairs
  from the `DagManifest`. No need to build live `DagNode`s (which require
  `NodeCtx`, `RuntimeEnv`, etc.). This means codegen works from a snapshot
  alone.
- **Additive, not invasive.** Extend `NodeFactory` with default-returning
  methods. Existing nodes keep working; codegen is implemented node-by-node.
- **Variable-flow tracking.** The compiler assigns a variable name to each
  `(node_id, output_port)` pair and resolves downstream input ports to
  upstream variable names, so generated code is a linear sequence of
  statements even for DAGs with branches and merges.

## Core types

### `CodegenTarget`

```rust
pub enum CodegenTarget { R, Python }
```

### `CodegenCtx` — per-node compilation context

Passed into each factory's codegen method. Holds the input variable names
(one per connected input port), the suggested output variable name, and a
fresh-name allocator for intermediate variables.

```rust
pub struct CodegenCtx<'a> {
    /// Variable names feeding each input port (index-aligned).
    pub input_vars: &'a [String],
    /// The variable name the compiler pre-allocated for this node's
    /// primary (port-0) output. Multi-output nodes allocate additional
    /// names via `fresh_var`.
    pub output_var: &'a str,
    pub target: CodegenTarget,
    fresh: usize,
}

impl CodegenCtx<'_> {
    pub fn fresh_var(&mut self, hint: &str) -> String {
        let name = format!("{hint}_{self.fresh}");
        self.fresh += 1;
        name
    }
}
```

### `NodeCodegen` — per-node result

```rust
pub struct NodeCodegen {
    /// Generated source lines (statements in the target language).
    pub code: Vec<String>,
    /// Variable names for each output port. `code[0]` → output port 0, etc.
    /// For single-output nodes: `vec![output_var.to_string()]`.
    pub output_vars: Vec<String>,
    /// Additional packages beyond the factory's static list (rarely needed).
    pub extra_packages: Vec<String>,
}
```

### `CodegenError`

```rust
#[derive(Debug, thiserror::Error)]
pub enum CodegenError {
    #[error("node kind '{kind}' does not support {target:?} codegen")]
    NotSupported { kind: String, target: CodegenTarget },
    #[error("failed to deserialize spec for '{kind}': {source}")]
    BadSpec { kind: String, source: serde_json::Error },
    #[error("DAG topology error: {0}")]
    Topology(String),
}
```

## Trait extension: `NodeFactory`

```rust
pub trait NodeFactory: Send + Sync {
    // ... existing methods (kind, desc, doc, spec_schema, ports, build) ...

    /// Compile this node kind's spec into R code.
    fn codegen_r(
        &self,
        spec: &serde_json::Value,
        ctx: &mut CodegenCtx,
    ) -> Result<NodeCodegen, CodegenError> {
        let _ = ctx;
        Err(CodegenError::NotSupported {
            kind: self.kind().to_string(),
            target: CodegenTarget::R,
        })
    }

    /// Compile this node kind's spec into Python code.
    fn codegen_python(
        &self,
        spec: &serde_json::Value,
        ctx: &mut CodegenCtx,
    ) -> Result<NodeCodegen, CodegenError> {
        let _ = ctx;
        Err(CodegenError::NotSupported {
            kind: self.kind().to_string(),
            target: CodegenTarget::Python,
        })
    }

    /// R packages this node's generated code requires (e.g. `["TwoSampleMR"]`).
    fn r_packages(&self) -> Vec<String> { vec![] }

    /// Python packages required.
    fn python_packages(&self) -> Vec<String> { vec![] }
}
```

Default implementations return `NotSupported`, so existing factories keep
working unchanged. Codegen is implemented incrementally, node kind by node
kind.

## `DagCompiler` — DAG-level orchestrator

```rust
pub struct DagCompiler<'a> {
    registry: &'a NodeRegistry,
}

pub struct CompiledScript {
    pub target: CodegenTarget,
    pub source: String,           // complete, ready-to-run script
    pub packages: Vec<String>,    // all required packages (deduped)
    pub skipped_nodes: Vec<String>, // kinds that returned NotSupported
}

impl DagCompiler<'_> {
    pub fn compile(
        &self,
        manifest: &DagManifest,
        target: CodegenTarget,
    ) -> Result<CompiledScript, CodegenError> {
        // 1. Build adjacency map from manifest edges
        //    child_to_parents: HashMap<NodeId, Vec<(NodeId, u8 /*from_port*/)>>
        //    keyed by (to_node, to_port)
        let edges_in = build_incoming_edge_map(&manifest.edges);

        // 2. Topological sort (Kahn's algorithm on the manifest's node set)
        let order = topo_sort(&manifest.nodes, &manifest.edges)?;

        // 3. Walk nodes in topo order
        let mut port_vars: HashMap<(NodeId, u8), String> = HashMap::new();
        let mut body: Vec<String> = Vec::new();
        let mut all_packages: HashSet<String> = HashSet::new();
        let mut skipped: Vec<String> = Vec::new();
        let mut fresh_counter = 0;

        for entry in &manifest.nodes {
            let factory = self.registry.get_factory(&entry.kind)?;

            // Resolve input variable names from upstream outputs
            let input_ports = factory.ports();
            let n_inputs = input_ports.input_ports().len();
            let mut input_vars: Vec<String> = Vec::with_capacity(n_inputs);
            for port_idx in 0..n_inputs as u8 {
                // Find the edge targeting (entry.id, port_idx)
                let upstream = edges_in.get(&(entry.id.clone(), port_idx));
                match upstream {
                    Some((from_node, from_port)) => {
                        let var = port_vars.get(&(*from_node.clone(), *from_port))
                            .cloned()
                            .unwrap_or_else(|| format!("__unresolved_{}", port_idx));
                        input_vars.push(var);
                    }
                    None => input_vars.push(format!("__missing_input_{}", port_idx)),
                }
            }

            // Pre-allocate output variable name
            let output_var = sanitize_var_name(&entry.id);

            let mut ctx = CodegenCtx {
                input_vars: &input_vars,
                output_var: &output_var,
                target,
                fresh: &mut fresh_counter,
            };

            // Delegate to factory
            let result = match target {
                CodegenTarget::R => factory.codegen_r(&entry.spec, &mut ctx),
                CodegenTarget::Python => factory.codegen_python(&entry.spec, &mut ctx),
            };

            match result {
                Ok(node_cg) => {
                    // Register output variable names for downstream nodes
                    for (port_idx, var) in node_cg.output_vars.iter().enumerate() {
                        port_vars.insert(
                            (entry.id.clone(), port_idx as u8),
                            var.clone(),
                        );
                    }
                    // Section header comment
                    body.push(String::new()); // blank line
                    body.push(format!(
                        "# ── {}: {} ──────────────────────────────────",
                        entry.id, entry.kind
                    ));
                    body.extend(node_cg.code);

                    // Collect packages
                    for pkg in factory.r_packages() {
                        all_packages.insert(pkg);
                    }
                    for pkg in node_cg.extra_packages {
                        all_packages.insert(pkg);
                    }
                }
                Err(CodegenError::NotSupported { kind, .. }) => {
                    skipped.push(kind.clone());
                    body.push(format!(
                        "# NOTE: node '{}' (kind '{}') has no {} codegen — skipped",
                        entry.id, entry.kind, format!("{target:?}").to_lowercase()
                    ));
                    // Still register a passthrough variable so downstream
                    // nodes have something to reference
                    port_vars.insert((entry.id.clone(), 0), output_var);
                }
                Err(e) => return Err(e),
            }
        }

        // 4. Assemble final script: header + body
        let header = match target {
            CodegenTarget::R => {
                let mut h = String::from("# Generated by autonomics DAG compiler\n\n");
                for pkg in &all_packages {
                    h.push_str(&format!("library({pkg})\n"));
                }
                h.push_str("library(data.table)\n\n");
                h
            }
            CodegenTarget::Python => {
                let mut h = String::from("# Generated by autonomics DAG compiler\n\n");
                for pkg in &all_packages {
                    h.push_str(&format!("import {pkg}\n"));
                }
                h.push_str("import pandas as pd\n\n");
                h
            }
        };

        let source = format!(
            "{header}\n{}",
            body.join("\n")
        );

        Ok(CompiledScript {
            target,
            source,
            packages: all_packages.into_iter().collect(),
            skipped_nodes: skipped,
        })
    }

    /// Convenience: compile the live DAG's current manifest.
    pub fn compile_dag(
        &self,
        dag: &DAG,
        target: CodegenTarget,
    ) -> Result<CompiledScript, CodegenError> {
        self.compile(&dag.to_manifest(), target)
    }
}
```

## Concrete codegen examples

### `LdscHsqNodeFactory::codegen_r`

Spec: `LdscHsqConfig { n_blocks: 200, intercept: None }`
Input port 0: DataFrame with columns `(z, n, rsid)`

```rust
fn codegen_r(
    &self,
    spec: &serde_json::Value,
    ctx: &mut CodegenCtx,
) -> Result<NodeCodegen, CodegenError> {
    let cfg: LdscHsqConfig = serde_json::from_value(spec.clone())
        .map_err(|e| CodegenError::BadSpec { kind: "ldsc".into(), source: e })?;

    let input = &ctx.input_vars[0];
    let out = ctx.output_var.to_string();
    let mut code = Vec::new();

    // Prepare sumstats in LDSC format
    let ldsc_df = ctx.fresh_var("ldsc_input");
    code.push(format!("{ldsc_df} <- data.frame("));
    code.push(format!("  rsid = {input}$rsid,"));
    code.push(format!("  Z = {input}$z,"));
    code.push(format!("  N = {input}$n"));
    code.push(format!(")"));

    // Run h² estimation
    code.push(format!("{out} <- ldsc::estimate_h2("));
    code.push(format!("  sumstats = {ldsc_df},"));
    code.push(format!("  n_blocks = {},", cfg.n_blocks));
    if let Some(v) = cfg.intercept {
        code.push(format!("  intercept = {v},"));
    }
    code.push(format!(")"));

    Ok(NodeCodegen {
        code,
        output_vars: vec![out],
        extra_packages: vec![],
    })
}

fn r_packages(&self) -> Vec<String> { vec!["LDSC".into()] }
```

### `TwoSampleMrNodeFactory::codegen_r`

Spec: `TwoSampleMrNodeSpec { id_exposure, id_outcome, method_list, clump, ... }`
Input port 0: merged exposure-outcome DataFrame (TwoSampleMR column conventions)

```rust
fn codegen_r(
    &self,
    spec: &serde_json::Value,
    ctx: &mut CodegenCtx,
) -> Result<NodeCodegen, CodegenError> {
    let spec: TwoSampleMrNodeSpec = serde_json::from_value(spec.clone())
        .map_err(|e| CodegenError::BadSpec { kind: "two_sample_mr".into(), source: e })?;

    let input = &ctx.input_vars[0];
    let out = ctx.output_var.to_string();
    let mut code = Vec::new();

    // TwoSampleMR expects formatted exposure/outcome data
    let exp_dat = ctx.fresh_var("exposure_dat");
    code.push(format!("# Format as TwoSampleMR exposure data"));
    code.push(format!("{exp_dat} <- data.frame("));
    code.push(format!("  SNP = {input}$snp,"));
    code.push(format!("  beta.exposure = {input}$beta_exposure,"));
    code.push(format!("  se.exposure = {input}$se_exposure,"));
    code.push(format!("  effect_allele.exposure = {input}$effect_allele_exposure,"));
    code.push(format!("  other_allele.exposure = {input}$other_allele_exposure,"));
    code.push(format!("  eaf.exposure = {input}$eaf_exposure,"));
    code.push(format!("  id.exposure = \"{}\",", spec.id_exposure));
    code.push(format!("  exposure = \"{}\"", spec.id_exposure));
    code.push(format!(")"));

    // LD clumping
    code.push(format!("# LD clumping via OpenGWAS"));
    code.push(format!("{exp_dat} <- clump_data({exp_dat},"));
    code.push(format!("  clump_r2 = {},", spec.clump.r2));
    code.push(format!("  clump_kb = {},", spec.clump.kb));
    code.push(format!("  clump_p1 = {},", spec.clump.p1));
    code.push(format!("  pop = \"{}\"", spec.clump.pop));
    code.push(format!(")"));

    // Outcome data
    let out_dat = ctx.fresh_var("outcome_dat");
    code.push(format!("{out_dat} <- data.frame("));
    code.push(format!("  SNP = {input}$snp,"));
    code.push(format!("  beta.outcome = {input}$beta_outcome,"));
    code.push(format!("  se.outcome = {input}$se_outcome,"));
    code.push(format!("  effect_allele.outcome = {input}$effect_allele_outcome,"));
    code.push(format!("  other_allele.outcome = {input}$other_allele_outcome,"));
    code.push(format!("  eaf.outcome = {input}$eaf_outcome,"));
    code.push(format!("  id.outcome = \"{}\",", spec.id_outcome));
    code.push(format!("  outcome = \"{}\"", spec.id_outcome));
    code.push(format!(")"));

    // Harmonise
    let harm = ctx.fresh_var("harmonised");
    code.push(format!("{harm} <- harmonise_data("));
    code.push(format!("  exposure_dat = {exp_dat},"));
    code.push(format!("  outcome_dat = {out_dat},"));
    code.push(format!("  action = {}", harmonise_action_to_r(spec.action)));
    code.push(format!(")"));

    // MR dispatch
    let methods = if spec.method_list.is_empty() {
        "mr_ivw, mr_egger_regression, mr_weighted_median, mr_simple_mode, mr_weighted_mode".to_string()
    } else {
        spec.method_list.iter()
            .map(|m| format!("\"{m}\""))
            .collect::<Vec<_>>()
            .join(", ")
    };
    code.push(format!("{out} <- mr({harm}, method_list = c({methods}))"));
    code.push(format!("print({out})"));

    Ok(NodeCodegen {
        code,
        output_vars: vec![out],
        extra_packages: vec![],
    })
}

fn r_packages(&self) -> Vec<String> { vec!["TwoSampleMR".into()] }
```

### `FileSourceNodeFactory::codegen_r`

```rust
fn codegen_r(
    &self,
    spec: &serde_json::Value,
    ctx: &mut CodegenCtx,
) -> Result<NodeCodegen, CodegenError> {
    let spec: FileSourceNodeSpec = serde_json::from_value(spec.clone())?;
    let out = ctx.output_var.to_string();
    let read_fn = match spec.format {
        FileFormat::Csv => "fread",
        FileFormat::Tsv => "fread",
        FileFormat::Parquet => "read_parquet",
    };
    let code = vec![
        format!("{out} <- {read_fn}(\"{}\")", spec.path),
    ];
    Ok(NodeCodegen {
        code,
        output_vars: vec![out],
        extra_packages: vec![],
    })
}

fn r_packages(&self) -> Vec<String> { vec!["data.table".into()] }
```

### `SqlNodeFactory::codegen_r`

SQL nodes are tricky — the generated R needs to replicate the SQL query.
For simple `SELECT ... WHERE ...` queries, emit `dplyr::filter` / `mutate`.
For complex queries, emit the raw SQL via `sqldf` or `DBI`.

```rust
fn codegen_r(
    &self,
    spec: &serde_json::Value,
    ctx: &mut CodegenCtx,
) -> Result<NodeCodegen, CodegenError> {
    let spec: SqlNodeSpec = serde_json::from_value(spec.clone())?;
    let out = ctx.output_var.to_string();
    let input = &ctx.input_vars[0];

    // Pragmatic approach: use sqldf to run the same SQL on the data.frame
    let code = vec![
        format!("# SQL node — replicating original query via sqldf"),
        format!("{out} <- sqldf(\"{}\", dbname = tempfile())",
            spec.sql_query.replace('"', "\\\"")),
    ];
    Ok(NodeCodegen {
        code,
        output_vars: vec![out],
        extra_packages: vec!["sqldf".into()],
    })
}
```

> **Note**: For SQL nodes with multiple inputs (variadic), the generated code
> would register each input as a named table in sqldf's environment.

## Agent tool: `compile_dag`

Exposed via `data-engine-tools` so the agent can trigger codegen from
conversation:

```rust
#[tool(
    name = "compile_dag",
    description = "Reverse-compile the current DAG into equivalent R or Python \
                  source code. The generated script calls the original reference \
                  packages (e.g. TwoSampleMR, LDSC, lava) and is fully runnable \
                  standalone. Use this to audit the analysis, cross-validate \
                  results, or generate publication-ready methodology code."
)]
pub struct CompileDagInput {
    /// Target language: "r" or "python".
    pub language: String,
    /// If provided, write the script to this path. Otherwise return as text.
    #[serde(default)]
    pub output_path: Option<String>,
}
```

## Differential testing strategy

The generated R scripts serve as golden reference implementations for CI:

```rust
#[cfg(test)]
mod diff_tests {
    use super::*;

    /// For each bio_crate with R codegen support, run the same DAG through:
    /// 1. The Rust engine (fast path)
    /// 2. Rscript on the generated R script (reference path)
    /// Compare key numeric outputs within a tolerance.
    #[tokio::test]
    #[ignore = "requires R + packages installed"]
    async fn ldsc_h2_rust_vs_r() {
        // Build a small test DAG
        let manifest = test_ldsc_manifest();
        let rust_result = run_dag_rust(&manifest).await;

        // Compile to R
        let compiler = DagCompiler { registry: &registry };
        let script = compiler.compile(&manifest, CodegenTarget::R)?;
        std::fs::write("/tmp/test_ldsc.R", &script.source)?;

        // Run R
        let r_output = std::process::Command::new("Rscript")
            .arg("/tmp/test_ldsc.R")
            .output()?;

        // Parse R output and compare
        let r_result = parse_r_ldsc_output(&r_output.stdout);
        assert!((rust_result.h2 - r_result.h2).abs() < 1e-6);
    }
}
```

## Implementation phases

### Phase 1: Core infrastructure (this PR)

- [ ] Define `CodegenTarget`, `CodegenCtx`, `NodeCodegen`, `CodegenError`
- [ ] Extend `NodeFactory` with `codegen_r`, `codegen_python`, `r_packages`, `python_packages`
- [ ] Implement `DagCompiler::compile`
- [ ] Topo-sort helper for manifests
- [ ] Variable-name sanitizer
- [ ] Unit tests: compile a 3-node DAG, verify variable flow

### Phase 2: Codegen for core node kinds

Priority order (by scientific impact + R-package availability):

- [ ] `source_file` → `fread()` / `read_parquet()`
- [ ] `sql` → `sqldf()` (pragmatic; full dplyr translation later)
- [ ] `sink_file` → `fwrite()` / `write_parquet()`
- [ ] `ldsc` (h²) → `LDSC::estimate_h2()` or Python `ldsc.py` CLI
- [ ] `ldsc_rg` → `LDSC::estimate_rg()`
- [ ] `two_sample_mr` → `TwoSampleMR::mr()` (full pipeline: clump → harmonise → dispatch)
- [ ] `linear_regression` → `lm()`
- [ ] `logistic_regression` → `glm(family = binomial)`
- [ ] `cox_regression` → `survival::coxph()`
- [ ] `lava_*` → `lava::()` family
- [ ] `susie_rss` → `susieR::susie_rss()`
- [ ] `mtag` → `MTAG::mtag()`
- [ ] `cpassoc` → `CPASSOC::cpassoc()`
- [ ] `magma_*` → MAGMA CLI commands
- [ ] `cmest_*` → `CMAverse::cmest()`
- [ ] `causal` → `tableone`/`MatchIt`/`survey` depending on method
- [ ] `mediation` → `mediation::mediate()`
- [ ] `epi_*` → respective R packages

### Phase 3: Python codegen

Mirror Phase 2 for Python (`pandas`, `statsmodels`, `lifelines`, `MendelianRandomization`, etc.).

### Phase 4: Differential testing in CI

- [ ] Wire `Rscript` execution into test harness
- [ ] Numeric tolerance comparison framework
- [ ] Per-crate golden fixture management

## Module layout

```
crates/data-engine/src/codegen/
├── mod.rs              // public API: DagCompiler, CompiledScript, CodegenTarget
├── context.rs          // CodegenCtx, NodeCodegen, CodegenError
├── compiler.rs         // DagCompiler::compile (topo sort + delegation)
└── tests.rs            // unit tests
```

No new crate — codegen lives inside `data-engine` because it needs access to
`NodeRegistry`, `DagManifest`, and `NodeFactory`. The agent tool lives in
`data-engine-tools`.

## Open questions

1. **SQL node fidelity.** `sqldf` works but adds a heavy dependency. For
   simple `SELECT ... WHERE` queries, a `dplyr` translation would be cleaner.
   Decision: start with `sqldf` (correctness first), add dplyr translation
   later for readability.

2. **Multi-input SQL nodes.** `SqlNode` is variadic — it can accept N input
   DataFrames. The generated code needs to register each input as a named
   table before the SQL runs. This requires the codegen to know the table
   aliases used inside the SQL query, which is not currently in the spec.
   Option: parse the SQL for table names; or add an optional `table_names`
   field to `SqlNodeSpec`.

3. **I/O path translation.** Rust nodes read from VFS tables
   (`vfs.gwas.bmi`) and virtual filesystem paths. The generated R script
   needs real filesystem paths or VFS R bindings (which don't exist yet).
   Decision: for VFS sources, emit a comment with the table name and a
   placeholder `# TODO: export vfs table to CSV first`. For file paths,
   pass through as-is (the Rust node already uses real paths).

4. **Node-specific auxiliary output.** Some Rust nodes produce diagnostic
   plots, intermediate statistics, or side-channel metadata that the R
   equivalent doesn't. The generated code should include `# NOTE:` comments
   documenting these gaps so the human auditor knows what's different.
