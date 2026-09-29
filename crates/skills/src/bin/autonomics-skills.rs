//! `autonomics-skills` — manage the skill library from the shell.
//!
//! Companion binary in the style of `autonomics-catalog`: thin clap
//! surface over the [`skills`] library. Tiers resolved as:
//! builtin (compiled in) + global `<state_dir>/skills`, with
//! `--workspace <dir>` optionally adding a shadowing workspace tier.

use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Parser, Subcommand};
use skills::registry::{SkillRegistry, SkillTier};
use skills::{default_state_dir, install};

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
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    let state_dir = cli.state_dir.clone().unwrap_or_else(default_state_dir);
    let registry = {
        let reg = SkillRegistry::standard(&state_dir);
        match &cli.workspace_dir {
            Some(dir) => reg.with_root(SkillTier::Workspace, dir),
            None => reg,
        }
    };

    let result = match cli.command {
        Command::List => list(&registry),
        Command::Show { name } => show(&registry, &name),
        Command::Install {
            source,
            workspace: to_workspace,
        } => install_cmd(
            &source,
            &state_dir,
            cli.workspace_dir.as_deref(),
            to_workspace,
        ),
        Command::Uninstall { name } => uninstall_cmd(&registry, &name),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("error: {e}");
            ExitCode::FAILURE
        }
    }
}

fn list(registry: &SkillRegistry) -> Result<(), String> {
    let entries = registry.list();
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

fn show(registry: &SkillRegistry, name: &str) -> Result<(), String> {
    let doc = registry
        .get(name)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("no skill named {name:?}"))?;
    println!("name: {}", doc.meta.name);
    println!("tier: {}", doc.tier.as_str());
    println!("---");
    println!("{}", doc.body);
    Ok(())
}

fn install_cmd(
    source: &str,
    state_dir: &std::path::Path,
    workspace_flag: Option<&std::path::Path>,
    to_workspace: bool,
) -> Result<(), String> {
    let dest_root = if to_workspace {
        workspace_flag.map(PathBuf::from).ok_or_else(|| {
            "--workspace <dir> is required to install into the workspace tier".to_string()
        })?
    } else {
        state_dir.join("skills")
    };
    println!("Installing from {source:?} into {} …", dest_root.display());

    // Disambiguation order mirrors the ecosystem convention: an
    // existing local path (after ~ expansion) always wins; only then
    // is the source treated as a git reference. A local `./pack` and
    // the shorthand `owner/repo` would otherwise both parse.
    let local_path = PathBuf::from(shellexpand_home(source));
    let outcome = if local_path.is_dir() {
        let record = install::InstallRecord {
            source: source.to_string(),
            commit: None,
            installed_at: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs() as i64)
                .unwrap_or(0),
        };
        install::install_from_local(&local_path, &dest_root, Some(&record))
    } else {
        install::install_from_git(source, &dest_root)
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

fn uninstall_cmd(registry: &SkillRegistry, name: &str) -> Result<(), String> {
    // Refuse builtin by checking the resolved tier first.
    if let Some(entry) = registry.list().into_iter().find(|e| e.name() == name) {
        if entry.tier == SkillTier::Builtin {
            return Err(skills::SkillError::Builtin(name.to_string()).to_string());
        }
    }
    let roots: Vec<PathBuf> = registry
        .roots()
        .iter()
        .map(|(_, root)| root.clone())
        // Higher tiers first: uninstall from where it actually lives.
        .rev()
        .collect();
    let removed = install::uninstall(name, &roots).map_err(|e| e.to_string())?;
    println!("removed {}", removed.display());
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
