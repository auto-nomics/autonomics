//! Subcommand entry points for the unified `autonomics` CLI.
//!
//! The deployment ships exactly one executable; these functions are the
//! implementations behind its `compute` subcommands. The binary wires
//! them with its own dispatcher:
//!
//! ```text
//! autonomics compute enroll <id> --login <alias> [--data-alias <a> --data-root <dir>]
//!                           --worker <cluster-path> [--partition P] [--account A]
//!                           [--ssh-command <path>] [--scratch <dir>] [--name NAME]
//!                           [--no-select] [--gateway ADDR] [--token TOKEN]
//! ```
//!
//! The gateway address/token default to `AUTONOMICS_HTTP_API_ADDR` and
//! `AUTONOMICS_HTTP_API_TOKEN` when the flags are absent.

use crate::GatewayClient;

/// The `autonomics compute enroll …` subcommand. `args` are the words
/// after `compute enroll`. Returns the process exit code.
pub async fn compute_enroll_main(args: &[String]) -> i32 {
    let mut id = None;
    let mut name = None;
    let mut login = None;
    let mut data_alias = None;
    let mut data_root = None;
    let mut worker = None;
    let mut deploy = None;
    let mut partition = None;
    let mut account = None;
    let mut ssh_command = None;
    let mut python = None;
    let mut scratch = None;
    let mut select = true;
    let mut gateway =
        std::env::var("AUTONOMICS_HTTP_API_ADDR").unwrap_or_else(|_| "127.0.0.1:8765".into());
    let mut token = std::env::var("AUTONOMICS_HTTP_API_TOKEN").ok();

    fn next_value(values: &mut std::slice::Iter<String>, flag: &str) -> String {
        match values.next() {
            Some(value) => value.clone(),
            None => {
                eprintln!("{flag} expects a value");
                std::process::exit(2);
            }
        }
    }

    let mut values = args.iter();
    while let Some(flag) = values.next() {
        // Closure borrow discipline: take values by &mut explicitly.
        match flag.as_str() {
            other if !other.starts_with("--") => {
                if id.is_none() {
                    id = Some(other.to_string());
                } else {
                    eprintln!("unexpected argument `{other}`");
                    std::process::exit(2);
                }
            }
            "--name" => name = Some(next_value(&mut values, flag)),
            "--login" => login = Some(next_value(&mut values, flag)),
            "--data-alias" => data_alias = Some(next_value(&mut values, flag)),
            "--data-root" => data_root = Some(next_value(&mut values, flag)),
            "--worker" => worker = Some(next_value(&mut values, flag)),
            "--deploy" => deploy = Some(next_value(&mut values, flag)),
            "--partition" => partition = Some(next_value(&mut values, flag)),
            "--account" => account = Some(next_value(&mut values, flag)),
            "--ssh-command" => ssh_command = Some(next_value(&mut values, flag)),
            "--python" => python = Some(next_value(&mut values, flag)),
            "--scratch" => scratch = Some(next_value(&mut values, flag)),
            "--gateway" => gateway = next_value(&mut values, flag),
            "--token" => token = Some(next_value(&mut values, flag)),
            "--no-select" => select = false,
            other => {
                eprintln!("unknown flag `{other}`");
                usage();
                std::process::exit(2);
            }
        }
    }

    let (Some(id), Some(login), Some(worker)) = (id, login, worker) else {
        usage();
        return 2;
    };
    let client = match GatewayClient::new(&gateway, token.as_deref()) {
        Ok(client) => client,
        Err(error) => {
            eprintln!("cannot reach the gateway at {gateway}: {error}");
            return 1;
        }
    };
    let status = match client.compute_status().await {
        Ok(status) => status,
        Err(error) => {
            eprintln!("cannot read compute registry state: {error}");
            return 1;
        }
    };
    let request = runtime::compute::EnrollRequest {
        expected_revision: status.state.revision,
        id,
        name,
        login_alias: login,
        data_alias,
        data_root,
        worker_executable: worker,
        deploy_worker: deploy,
        partition,
        account,
        ssh_command,
        python,
        scratch_dir: scratch,
        select,
    };
    match client.compute_enroll(&request).await {
        Ok(state) => {
            let selected = if select { " (selected)" } else { "" };
            println!(
                "enrolled `{}` at revision {}{selected}",
                request.id,
                state
                    .targets
                    .iter()
                    .filter(|t| t.id == request.id)
                    .map(|t| t.revision)
                    .max()
                    .unwrap_or_default()
            );
            0
        }
        Err(error) => {
            eprintln!("enrollment failed: {error}");
            1
        }
    }
}

fn usage() {
    eprintln!("usage: autonomics compute enroll <id> --login <ssh-alias> --worker <cluster-path>");
    eprintln!("       [--data-alias <a> --data-root <dir>] [--partition P] [--account A]");
    eprintln!("       [--deploy <local-binary>]   # stream the worker to --worker first");
    eprintln!("       [--python <cluster-python>] # helper interpreter when system python < 3.7");
    eprintln!("       [--ssh-command <path>] [--scratch <dir>] [--name NAME] [--no-select]");
    eprintln!("       [--gateway ADDR] [--token TOKEN]");
}
