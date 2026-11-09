//! Subcommand entry points for the unified `autonomics` CLI.
//!
//! The coordinator CLI manages the separately built `autonomics-worker`.
//! These functions implement its `compute` subcommands. The binary wires
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

/// Inspect a target's configuration and perform a read-only connection probe.
pub async fn compute_check_main(args: &[String]) -> i32 {
    let mut id = None;
    let mut gateway = std::env::var("AUTONOMICS_HTTP_API_ADDR")
        .unwrap_or_else(|_| "127.0.0.1:8765".into());
    let mut token = std::env::var("AUTONOMICS_HTTP_API_TOKEN").ok();
    let mut values = args.iter();
    while let Some(arg) = values.next() {
        match arg.as_str() {
            "--gateway" | "--token" => {
                let Some(value) = values.next() else {
                    eprintln!("{arg} expects a value");
                    return 2;
                };
                if arg == "--gateway" { gateway = value.clone(); } else { token = Some(value.clone()); }
            }
            value if !value.starts_with('-') && id.is_none() => id = Some(value.to_owned()),
            _ => { eprintln!("usage: autonomics compute check <id> [--gateway ADDR] [--token TOKEN]"); return 2; }
        }
    }
    let Some(id) = id else {
        eprintln!("usage: autonomics compute check <id> [--gateway ADDR] [--token TOKEN]");
        return 2;
    };
    let client = match GatewayClient::new(&gateway, token.as_deref()) {
        Ok(client) => client,
        Err(error) => { eprintln!("invalid gateway configuration: {error}"); return 1; }
    };
    match client.check_compute_target(&id).await {
        Ok(result) => {
            println!("{}", serde_json::to_string_pretty(&result).unwrap());
            if result["configuration_valid"] == false { 1 } else { 0 }
        }
        Err(error) => { eprintln!("compute target check failed: {error}"); 1 }
    }
}

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
    eprintln!("[gateway] 正在读取计算目标配置…");
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
    eprintln!(
        "[enroll] 开始注册 {}（{}）",
        request.id, request.login_alias
    );
    let started = std::time::Instant::now();
    let last_stage = std::cell::RefCell::new(String::from("gateway"));
    let operation = client.compute_enroll_with_progress(&request, |stage, message| {
        *last_stage.borrow_mut() = stage.to_owned();
        eprintln!(
            "[{:>4}s] [{stage}] {}",
            started.elapsed().as_secs(),
            message.escape_debug()
        );
    });
    tokio::pin!(operation);
    let mut heartbeat = tokio::time::interval(std::time::Duration::from_secs(15));
    heartbeat.tick().await;
    let result = loop {
        tokio::select! {
            result = &mut operation => break result,
            _ = heartbeat.tick() => eprintln!("[{:>4}s] [{}] 仍在等待响应…", started.elapsed().as_secs(), last_stage.borrow()),
        }
    };
    match result {
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
    eprintln!(
        "       [--deploy <local-binary>]   # upload target/release/autonomics-worker to --worker"
    );
    eprintln!("       [--python <cluster-python>] # helper interpreter when system python < 3.7");
    eprintln!("       [--ssh-command <path>] [--scratch <dir>] [--name NAME] [--no-select]");
    eprintln!("       [--gateway ADDR] [--token TOKEN]");
}
