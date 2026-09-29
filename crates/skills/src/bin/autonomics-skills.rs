//! `autonomics-skills` — manage the skill library from the shell.
//!
//! Companion binary in the style of `autonomics-catalog`: thin clap
//! surface over the [`skills`] library. Tiers resolved as:
//! builtin (compiled in) + global `<state_dir>/skills`, with
//! `--workspace <dir>` optionally adding a shadowing workspace tier.

use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Parser, Subcommand};
use skills::SkillManager;

#[derive(Parser)]
#[command(
    name = "autonomics-skills",
    version,
    about = "Install, list, inspect, and remove agent skills."
)]
struct Cli {
    /// Use a workspace skills directory that shadows the global tier.
    /// Renamed from `--workspace` to avoid colliding with the Install
    /// subcommand's `--workspace` targeting flag.
    #[arg(long, global = true)]
    workspace_dir: Option<PathBuf>,

    /// Override the state directory (default $AUTONOMICS_STATE_DIR or ~/.autonomics).
    #[arg(long, global = true)]
    state_dir: Option<PathBuf>,

    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// List installed skills across all tiers.
    List,
    /// Print one skill's full SKILL.md.
    Show { name: String },
    /// Install from a local path or GitHub URL/shorthand.
    Install {
        /// Local directory, https://github.com/owner/repo[...], or owner/repo[@path].
        source: String,
        /// Install into the workspace tier instead of global.
        #[arg(long)]
        workspace: bool,
    },
    /// Remove an installed skill by name (global or workspace tier).
    Uninstall { name: String },
    /// List a skill's workflow templates and their parameter schemas.
    Workflows { name: String },
    /// Validate a skill directory before installing: SKILL.md format,
    /// every workflow template, every eval file.
    Validate { path: PathBuf },
    /// Show the in-process usage telemetry (empty for a fresh process;
    /// meaningful inside the daemon, exposed here for smoke tests).
    Usage,
    /// Record one observation feeding the skill evolution loop.
    Observe {
        /// One line a future search would find.
        #[arg(short, long)]
        summary: String,
        /// The reusable pattern, fix, or condition.
        #[arg(short, long)]
        body: String,
        /// failure | recipe | caveat (default failure).
        #[arg(short, long)]
        kind: Option<String>,
        /// Node kind anchor, when applicable.
        #[arg(long)]
        node_kind: Option<String>,
        /// Error text anchor, when applicable.
        #[arg(long)]
        error: Option<String>,
    },
    /// Cluster anchored observations and write skill proposals for
    /// repeated patterns (>= 3 occurrences, deterministic).
    Distill,
    /// List proposals, optionally filtered by status.
    Proposals { status: Option<String> },
    /// Approve a pending proposal into the global tier.
    Approve { name: String },
    /// Reject a pending proposal; its pattern will not re-propose.
    Reject { name: String },
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    let state_dir = cli
        .state_dir
        .clone()
        .unwrap_or_else(skills::default_state_dir);
    // A one-shot process has no cross-thread subscribers, but routing
    // mutations through the manager keeps the generation/notify
    // invariant identical to the daemon's.
    let manager = {
        let m = SkillManager::new(&state_dir);
        match &cli.workspace_dir {
            Some(dir) => m.with_workspace_root(dir),
            None => m,
        }
    };

    let result = match cli.command {
        Command::List => list(&manager),
        Command::Show { name } => show(&manager, &name),
        Command::Install {
            source,
            workspace: to_workspace,
        } => install_cmd(&manager, &source, to_workspace),
        Command::Uninstall { name } => uninstall_cmd(&manager, &name),
        Command::Workflows { name } => workflows_cmd(&manager, &name),
        Command::Validate { path } => validate_cmd(&path),
        Command::Usage => usage_cmd(&manager),
        Command::Observe {
            summary,
            body,
            kind,
            node_kind,
            error,
        } => observe_cmd(&manager, &summary, &body, kind, node_kind, error),
        Command::Distill => distill_cmd(&manager),
        Command::Proposals { status } => proposals_cmd(&manager, status.as_deref()),
        Command::Approve { name } => approve_cmd(&manager, &name),
        Command::Reject { name } => reject_cmd(&manager, &name),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("error: {e}");
            ExitCode::FAILURE
        }
    }
}

fn list(manager: &SkillManager) -> Result<(), String> {
    let entries = manager.registry().list();
    if entries.is_empty() {
        println!("No skills installed.");
        return Ok(());
    }
    println!("{} skill(s):", entries.len());
    for entry in &entries {
        let tags = if entry.meta.tags.is_empty() {
            String::new()
        } else {
            format!(" [{}]", entry.meta.tags.join(", "))
        };
        println!(
            "- {} ({}){} — {}",
            entry.meta.name,
            entry.tier.as_str(),
            tags,
            entry.meta.description
        );
    }
    Ok(())
}

fn show(manager: &SkillManager, name: &str) -> Result<(), String> {
    let doc = manager
        .registry()
        .get(name)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("no skill named {name:?}"))?;
    println!("name: {}", doc.meta.name);
    println!("tier: {}", doc.tier.as_str());
    println!("---");
    println!("{}", doc.body);
    Ok(())
}

fn install_cmd(manager: &SkillManager, source: &str, to_workspace: bool) -> Result<(), String> {
    if to_workspace {
        return Err(
            "workspace-tier installs are a daemon-side operation; the CLI \
             installs into the global tier (pass --workspace-dir at the \
             manager level instead)"
                .to_string(),
        );
    }
    println!("Installing from {source:?} …");

    // Disambiguation order mirrors the ecosystem convention: an
    // existing local path (after ~ expansion) always wins; only then
    // is the source treated as a git reference. A local `./pack` and
    // the shorthand `owner/repo` would otherwise both parse.
    let local_path = PathBuf::from(shellexpand_home(source));
    let outcome = if local_path.is_dir() {
        manager.install_local(&local_path)
    } else {
        manager.install_git(source)
    }
    .map_err(|e| e.to_string())?;

    for skill in &outcome.installed {
        println!("installed {} → {}", skill.name, skill.path.display());
    }
    for (dir, err) in &outcome.failed {
        eprintln!("failed  {dir}: {err}");
    }
    if !outcome.success() {
        return Err("no skills installed".into());
    }
    Ok(())
}

fn uninstall_cmd(manager: &SkillManager, name: &str) -> Result<(), String> {
    // Refuse builtin by checking the resolved tier first.
    if let Some(entry) = manager
        .registry()
        .list()
        .into_iter()
        .find(|e| e.name() == name)
        && entry.tier == skills::SkillTier::Builtin
    {
        return Err(skills::SkillError::Builtin(name.to_string()).to_string());
    }
    let removed = manager.uninstall(name).map_err(|e| e.to_string())?;
    println!(
        "removed {} (generation {})",
        removed.display(),
        manager.generation()
    );
    Ok(())
}

fn shellexpand_home(source: &str) -> String {
    if let Some(rest) = source.strip_prefix("~/") {
        if let Some(home) = std::env::var_os("HOME") {
            return PathBuf::from(home).join(rest).display().to_string();
        }
    }
    source.to_string()
}

fn workflows_cmd(manager: &SkillManager, name: &str) -> Result<(), String> {
    let doc = manager
        .registry()
        .get(name)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("no skill named {name:?}"))?;
    if doc.workflows.is_empty() {
        println!("Skill {name:?} bundles no workflow templates.");
        return Ok(());
    }
    let dir = doc
        .dir
        .ok_or_else(|| format!("skill {name:?} is builtin; no workflows"))?;
    for stem in &doc.workflows {
        match skills::WorkflowTemplate::load(&dir, stem) {
            Ok(template) => {
                println!("workflow {stem} — {}", template.description);
                println!(
                    "  nodes: {}",
                    template
                        .nodes
                        .iter()
                        .map(|n| n.id.as_str())
                        .collect::<Vec<_>>()
                        .join(", ")
                );
                for (pname, spec) in &template.params {
                    println!(
                        "  param {pname}: {} ({}{})",
                        spec.kind,
                        if spec.required {
                            "required"
                        } else {
                            "optional"
                        },
                        spec.default
                            .as_ref()
                            .map(|d| format!(", default {d}"))
                            .unwrap_or_default()
                    );
                }
            }
            Err(e) => eprintln!("workflow {stem}: INVALID — {e}"),
        }
    }
    if !doc.evals.is_empty() {
        println!("evals: {}", doc.evals.join(", "));
    }
    Ok(())
}

/// Author-side validation: everything a skill declares must parse and
/// hold together before it is installed or published.
fn validate_cmd(path: &std::path::Path) -> Result<(), String> {
    if !path.is_dir() {
        return Err(format!("{} is not a directory", path.display()));
    }
    let mut problems: Vec<String> = Vec::new();

    let (meta, _body) =
        skills::format::parse_skill_file(&path.join("SKILL.md")).map_err(|e| e.to_string())?;
    match skills::format::validate_meta(&meta) {
        Ok(()) => println!("SKILL.md: ok (name {:?})", meta.name),
        Err(e) => problems.push(format!("SKILL.md: {e}")),
    }

    let workflow_stems = skills::WorkflowTemplate::stems_in_dir(path);
    let mut known = Vec::new();
    for stem in &workflow_stems {
        match skills::WorkflowTemplate::load(path, stem) {
            Ok(template) => {
                println!(
                    "workflow {stem}: ok ({} node(s), {} param(s))",
                    template.nodes.len(),
                    template.params.len()
                );
                known.push(stem.clone());
            }
            Err(e) => problems.push(format!("workflow {stem}: {e}")),
        }
    }

    for stem in skills::eval::eval_stems_in_dir(path) {
        match skills::eval::load_cases(path, &stem) {
            Ok(cases) => {
                for case in &cases {
                    if !known.contains(&case.workflow) {
                        problems.push(format!(
                            "eval {stem:?} case {:?}: references unknown workflow {:?}",
                            case.name, case.workflow
                        ));
                    } else {
                        println!("eval {stem} case {}: ok", case.name);
                    }
                }
            }
            Err(e) => problems.push(format!("eval {stem}: {e}")),
        }
    }

    if !problems.is_empty() {
        eprintln!("\n{} problem(s):", problems.len());
        for p in &problems {
            eprintln!("  - {p}");
        }
        return Err("validation failed".into());
    }
    println!("\n{} valid.", meta.name);
    Ok(())
}

fn usage_cmd(manager: &SkillManager) -> Result<(), String> {
    let rows = manager.usage_snapshot();
    if rows.is_empty() {
        println!("No usage recorded (telemetry lives in the daemon process).");
        return Ok(());
    }
    // Header is fully static — no format args needed.
    println!(
        "skill                   {:>6} {:>6} {:>6} {:>6}  last_used",
        "get", "search", "run", "eval"
    );
    for (name, u) in rows {
        println!(
            "{:<24} {:>6} {:>6} {:>6} {:>6}  {}",
            name,
            u.gets,
            u.search_hits,
            u.runs,
            u.evals,
            if u.last_used == 0 {
                "-".to_string()
            } else {
                u.last_used.to_string()
            }
        );
    }
    println!("generation: {}", manager.generation());
    Ok(())
}

fn observe_cmd(
    manager: &SkillManager,
    summary: &str,
    body: &str,
    kind: Option<String>,
    node_kind: Option<String>,
    error: Option<String>,
) -> Result<(), String> {
    let kind = match kind.as_deref() {
        None => skills::ObservationKind::Failure,
        Some("failure") => skills::ObservationKind::Failure,
        Some("recipe") => skills::ObservationKind::Recipe,
        Some("caveat") => skills::ObservationKind::Caveat,
        Some(other) => return Err(format!("unknown kind {other:?} (failure|recipe|caveat)")),
    };
    let observation = manager
        .record_observation(skills::ObservationInput {
            kind,
            source: skills::ObservationSource::Cli,
            summary: summary.into(),
            body: body.into(),
            node_kind,
            error,
        })
        .map_err(|e| e.to_string())?;
    println!("recorded {}", observation.id);
    Ok(())
}

fn distill_cmd(manager: &SkillManager) -> Result<(), String> {
    let report = manager.distill().map_err(|e| e.to_string())?;
    println!(
        "{} candidate cluster(s) considered, {} proposal(s) written",
        report.candidates_considered,
        report.proposals_written.len()
    );
    for name in &report.proposals_written {
        println!("  proposed {name} (pending review)");
    }
    for (hash, reason) in &report.skipped {
        println!("  skipped {hash}: {reason}");
    }
    if report.proposals_written.is_empty() {
        println!(
            "\nNo new patterns. Observations cluster at >= {} anchored \
             occurrences; record more via `observe` or let failing evals \
             accumulate.",
            skills::distill::MIN_CLUSTER
        );
    } else {
        println!("\nReview with `proposals`, then `approve <name>` or `reject <name>`.");
    }
    Ok(())
}

fn proposals_cmd(manager: &SkillManager, status: Option<&str>) -> Result<(), String> {
    let proposals = manager.proposals().list();
    let filtered: Vec<_> = proposals
        .iter()
        .filter(|p| status.is_none_or(|s| p.status.as_str() == s))
        .collect();
    if filtered.is_empty() {
        println!(
            "No proposals{}.",
            status.map(|s| format!(" ({s})")).unwrap_or_default()
        );
        return Ok(());
    }
    for proposal in &filtered {
        println!(
            "{} [{}] — {}",
            proposal.name,
            proposal.status.as_str(),
            if proposal.rationale.is_empty() {
                "(no rationale)".to_string()
            } else {
                proposal.rationale.clone()
            }
        );
        println!(
            "  cluster {} from {} observation(s)",
            proposal.cluster_hash,
            proposal.source_observation_ids.len()
        );
    }
    Ok(())
}

fn approve_cmd(manager: &SkillManager, name: &str) -> Result<(), String> {
    let outcome = manager.approve_proposal(name).map_err(|e| e.to_string())?;
    println!(
        "approved {} → {} (generation {})",
        outcome.name,
        outcome.destination.display(),
        manager.generation()
    );
    Ok(())
}

fn reject_cmd(manager: &SkillManager, name: &str) -> Result<(), String> {
    manager.reject_proposal(name).map_err(|e| e.to_string())?;
    println!("rejected {name}; its pattern will not re-propose");
    Ok(())
}
