//! CLI parsing tests — wired into `cli.rs` via `#[path]` (the
//! `commands/run_tests.rs` convention). The app is binary-only, so the
//! tests construct the parser directly with `try_parse_from`.

use clap::Parser;

use crate::cli::{Cli, Command};

fn parse(args: &[&str]) -> Result<Cli, clap::Error> {
    Cli::try_parse_from(std::iter::once("autonomics").chain(args.iter().copied()))
}

#[test]
fn bare_invocation_has_no_command() {
    let cli = parse(&[]).unwrap();
    assert!(cli.command.is_none(), "the guide path handles this");
}

#[test]
fn prepare_parses_with_and_without_check() {
    let Command::Prepare(args) = parse(&["prepare"]).unwrap().command.unwrap() else {
        panic!("prepare subcommand");
    };
    assert!(!args.check);

    let Command::Prepare(args) = parse(&["prepare", "--check"]).unwrap().command.unwrap() else {
        panic!("prepare --check subcommand");
    };
    assert!(args.check);
}

#[test]
fn tui_parses_its_flags() {
    let Command::Tui(args) = parse(&["tui", "--nocapture"]).unwrap().command.unwrap() else {
        panic!("tui subcommand");
    };
    assert!(args.nocapture);
    assert!(args.config.is_none());
}

#[test]
fn serve_parses_daemon_flag_and_actions() {
    let Command::Serve(args) = parse(&["serve", "--daemon"]).unwrap().command.unwrap() else {
        panic!("serve subcommand");
    };
    assert!(args.daemon);
    assert!(args.action.is_none());

    let Command::Serve(args) = parse(&["serve", "status"]).unwrap().command.unwrap() else {
        panic!("serve status subcommand");
    };
    assert!(!args.daemon);
    assert!(matches!(args.action, Some(crate::cli::ServeAction::Status)));
}

#[test]
fn serve_start_is_not_a_subcommand() {
    // The dead `Start` variant was removed; `serve start` must be a
    // parse error, not a runtime panic.
    let error = parse(&["serve", "start"]).unwrap_err();
    assert!(
        error.to_string().to_lowercase().contains("unrecognized"),
        "{error}"
    );
}

#[test]
fn clap_metadata_is_self_consistent() {
    // Panics on command-name collisions, missing helps, or invalid
    // argument syntax — the debug assertions clap builds for tests.
    use clap::CommandFactory;
    Cli::command().debug_assert();
}
