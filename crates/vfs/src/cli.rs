//! Subcommand entry points for the unified `autonomics` CLI.
//!
//! The deployment ships exactly one executable (`/app/autonomics`). These
//! functions are the implementations behind its `vault` and `askpass`
//! subcommands; the binary wires them with a clap/c-style dispatcher of
//! its choosing:
//!
//! ```text
//! autonomics vault set <alias> [--password] [--otp] [--digits N] [--period S]
//! autonomics vault list
//! autonomics vault check
//! autonomics askpass <prompt>        # invoked by OpenSSH via SSH_ASKPASS
//! ```
//!
//! The vault file itself lives at a system-fixed location (the daemon's
//! state directory); the caller injects that path — the CLI never
//! accepts an arbitrary vault path, so a misdirected write cannot
//! silently fork the credential store.
//!
//! No standalone credential binaries exist on purpose.

use std::path::{Path, PathBuf};

use crate::credential::{CredentialVault, SshCredential};

/// The `autonomics askpass <prompt>` subcommand: bridge one OpenSSH
/// prompt to the daemon's vault agent. Prints the answer on stdout and
/// returns 0; any failure prints nothing and returns 1 so ssh fails
/// fast (BatchMode semantics are preserved for uncovered prompts).
pub fn askpass_main() -> i32 {
    use std::io::{Read, Write};
    use std::os::unix::net::UnixStream;

    let prompt = std::env::args().nth(2).unwrap_or_default();
    let Ok(socket) = std::env::var(crate::ssh_channel::VAULT_SOCK_ENV) else {
        return 1;
    };
    let alias = std::env::var(crate::ssh_channel::VAULT_ALIAS_ENV).unwrap_or_default();
    let request = serde_json::json!({"alias": alias, "prompt": prompt});
    let Ok(mut stream) = UnixStream::connect(&socket) else {
        return 1;
    };
    let mut line = serde_json::to_string(&request).unwrap_or_default();
    line.push('\n');
    if stream.write_all(line.as_bytes()).is_err() {
        return 1;
    }
    let mut response = String::new();
    if stream.read_to_string(&mut response).is_err() {
        return 1;
    }
    if let Ok(value) = serde_json::from_str::<serde_json::Value>(response.trim_end()) {
        if let Some(answer) = value["answer"].as_str() {
            println!("{answer}");
            return 0;
        }
    }
    1
}

/// The `autonomics vault …` subcommand. `vault_path` is the system-fixed
/// vault location; `args` are the words after `vault` (for example
/// `["set", "hpc-otp", "--otp"]`). The CLI never accepts an arbitrary
/// vault path — a misdirected write cannot silently fork the store.
pub fn vault_main(vault_path: &Path, args: &[String]) -> i32 {
    let Some(command) = args.first() else {
        vault_usage();
        return 2;
    };
    match command.as_str() {
        "set" => match args.get(1) {
            Some(alias) => vault_set(vault_path, alias, &args[2..]),
            None => {
                vault_usage();
                2
            }
        },
        "list" => vault_list(vault_path),
        "check" => vault_check(vault_path),
        _ => {
            vault_usage();
            2
        }
    }
}

fn vault_usage() {
    eprintln!("usage: autonomics vault set <alias> [--password] [--otp] [--digits N] [--period S]");
    eprintln!("       autonomics vault list");
    eprintln!("       autonomics vault check");
}

fn prompt_secret(label: &str) -> String {
    let value = rpassword::prompt_password(format!("{label}: ")).unwrap_or_default();
    if value.is_empty() {
        eprintln!("{label} must not be empty");
        std::process::exit(1);
    }
    value
}

fn vault_set(path: &Path, alias: &str, args: &[String]) -> i32 {
    let mut want_password = false;
    let mut want_otp = false;
    let mut digits = 6u32;
    let mut period = 30u64;
    let mut index = 0;
    while index < args.len() {
        match args[index].as_str() {
            "--password" => want_password = true,
            "--otp" => want_otp = true,
            "--digits" => {
                index += 1;
                match args.get(index).and_then(|value| value.parse().ok()) {
                    Some(value) if (1..=10).contains(&value) => digits = value,
                    _ => {
                        eprintln!("--digits expects 1-10");
                        return 2;
                    }
                }
            }
            "--period" => {
                index += 1;
                match args.get(index).and_then(|value| value.parse().ok()) {
                    Some(value) if value > 0 => period = value,
                    _ => {
                        eprintln!("--period expects a positive number of seconds");
                        return 2;
                    }
                }
            }
            _ => {
                vault_usage();
                return 2;
            }
        }
        index += 1;
    }
    if !want_password && !want_otp {
        want_password = true;
        want_otp = true;
    }
    let mut entry = SshCredential {
        totp_digits: digits,
        totp_period: period,
        ..Default::default()
    };
    println!("Adding credentials for `{alias}`:");
    if want_password {
        entry.password = Some(prompt_secret(&format!("  cluster password for `{alias}`")));
    }
    if want_otp {
        eprintln!(
            "  (the base32 string from the cluster's OTP enrollment page / QR manual-entry code)"
        );
        let secret = prompt_secret("  OTP shared secret (base32)");
        // Double entry: the prompts are echo-free, so a silent typo would
        // otherwise surface only as permanently wrong codes at login.
        let confirm = prompt_secret("  confirm OTP secret");
        if secret != confirm {
            eprintln!("OTP secrets do not match");
            return 1;
        }
        // Refuse to store a secret that cannot produce codes.
        if let Err(error) =
            crate::credential::totp_code(&secret, digits, period, std::time::SystemTime::now())
        {
            eprintln!("invalid OTP secret: {error}");
            return 1;
        }
        entry.totp_secret = Some(secret);
    }
    // Open the existing vault with its own passphrase, or create the vault
    // and set its passphrase once — an existing vault keeps its passphrase;
    // `set` never silently changes it.
    let (mut vault, passphrase) = match std::fs::read(path) {
        Ok(sealed) => {
            println!("Opening the existing vault at {}:", path.display());
            let passphrase = prompt_secret("  vault passphrase (unlocks the file)");
            match CredentialVault::open(&sealed, &passphrase) {
                Ok(vault) => (vault, passphrase),
                Err(error) => {
                    eprintln!("unlock failed: {error}");
                    return 1;
                }
            }
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            println!(
                "Creating a new vault at {} — choose a passphrase to encrypt it:",
                path.display()
            );
            eprintln!(
                "  (the passphrase protects the file at rest; a lost passphrase cannot be recovered)"
            );
            let passphrase = prompt_secret("  new vault passphrase");
            let confirm = prompt_secret("  confirm vault passphrase");
            if passphrase != confirm {
                eprintln!("passphrases do not match");
                return 1;
            }
            (CredentialVault::default(), passphrase)
        }
        Err(error) => {
            eprintln!("cannot read the vault: {error}");
            return 1;
        }
    };
    vault.entries.insert(alias.to_string(), entry);
    let sealed = match vault.seal(&passphrase) {
        Ok(sealed) => sealed,
        Err(error) => {
            eprintln!("sealing failed: {error}");
            return 1;
        }
    };
    if let Err(error) = vault_write_private(path, &sealed) {
        eprintln!("cannot write vault: {error}");
        return 1;
    }
    println!(
        "vault updated: {} entries ({})",
        vault.entries.len(),
        path.display()
    );
    0
}

fn vault_list(path: &Path) -> i32 {
    if path.is_file() {
        println!(
            "Listing {} requires its passphrase (entry names are encrypted):",
            path.display()
        );
    }
    let vault = vault_load_or_empty(path);
    if vault.entries.is_empty() {
        println!("(empty vault)");
        return 0;
    }
    for (alias, entry) in &vault.entries {
        let materials = [
            entry.password.as_ref().map(|_| "password"),
            entry.totp_secret.as_ref().map(|_| "otp"),
        ];
        let materials: Vec<&str> = materials.into_iter().flatten().collect();
        println!("{:<24} [{}]", alias, materials.join(","));
    }
    0
}

fn vault_check(path: &Path) -> i32 {
    let sealed = match std::fs::read(path) {
        Ok(sealed) => sealed,
        Err(error) => {
            eprintln!("cannot read vault: {error}");
            return 1;
        }
    };
    let passphrase = prompt_secret("vault passphrase (unlocks the file)");
    match CredentialVault::open(&sealed, &passphrase) {
        Ok(vault) => {
            println!("unlocked: {} entries", vault.entries.len());
            0
        }
        Err(error) => {
            eprintln!("unlock failed: {error}");
            1
        }
    }
}

fn vault_load_or_empty(path: &Path) -> CredentialVault {
    match std::fs::read(path) {
        Ok(sealed) => {
            let passphrase = prompt_secret("  vault passphrase (unlocks the file)");
            CredentialVault::open(&sealed, &passphrase).unwrap_or_else(|error| {
                eprintln!("unlock failed: {error}");
                std::process::exit(1);
            })
        }
        Err(_) => CredentialVault::default(),
    }
}

fn vault_write_private(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    std::fs::write(path, bytes)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
    }
    Ok(())
}
