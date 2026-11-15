use crate::{CompileOutput, Diagnostic};
use std::{
    path::{Path, PathBuf},
    time::UNIX_EPOCH,
};
use tectonic::driver::{OutputFormat, ProcessingSessionBuilder};
use tectonic_bridge_core::{SecuritySettings, SecurityStance};
use tectonic_status_base::{MessageKind, StatusBackend};

#[derive(Default)]
struct Status {
    log: String,
    diagnostics: Vec<Diagnostic>,
    had_error: bool,
}
fn append_limited(target: &mut String, text: &str, max: usize) {
    let mut end = text.len().min(max.saturating_sub(target.len()));
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    target.push_str(&text[..end]);
}
impl StatusBackend for Status {
    fn report(
        &mut self,
        kind: MessageKind,
        args: std::fmt::Arguments,
        err: Option<&tectonic::Error>,
    ) {
        let mut message = format!(
            "{args}{}",
            err.map(|e| format!(": {e}")).unwrap_or_default()
        );
        append_limited(&mut self.log, &message, 1024 * 1024);
        append_limited(&mut self.log, "\n", 1024 * 1024);
        if matches!(kind, MessageKind::Error) {
            self.had_error = true;
        }
        if self.diagnostics.len() >= 256 {
            if matches!(kind, MessageKind::Error) {
                self.diagnostics.remove(0);
            } else {
                return;
            }
        }
        if message.len() > 4096 {
            let mut end = 4096;
            while !message.is_char_boundary(end) {
                end -= 1;
            }
            message.truncate(end);
        }
        let severity = match kind {
            MessageKind::Error => "error",
            MessageKind::Warning => "warning",
            _ => "info",
        };
        self.diagnostics.push(Diagnostic {
            severity: severity.into(),
            message,
            file: None,
            line: None,
        });
    }
    fn dump_error_logs(&mut self, output: &[u8]) {
        if self.log.len() < 1024 * 1024 {
            append_limited(
                &mut self.log,
                &String::from_utf8_lossy(&output[..output.len().min(1024 * 1024)]),
                1024 * 1024,
            );
        }
    }
}
#[cfg(target_os = "linux")]
fn sandbox(project: &Path, cache: &Path, bundle: &str) -> Result<(), Box<dyn std::error::Error>> {
    use landlock::{
        ABI, Access, AccessFs, PathBeneath, PathFd, Ruleset, RulesetAttr, RulesetCreatedAttr,
        RulesetStatus, path_beneath_rules,
    };
    let access = AccessFs::from_all(ABI::V3);
    let mut rules = Ruleset::default()
        .handle_access(access)?
        .create()?
        .add_rule(PathBeneath::new(PathFd::new(project)?, access))?
        .add_rule(PathBeneath::new(PathFd::new(cache)?, access))?;
    // Network bundle transport may read public resolver and certificate configuration.
    for path in [
        "/etc/resolv.conf",
        "/etc/hosts",
        "/etc/nsswitch.conf",
        "/etc/ssl/certs",
        "/etc/ssl/certs/ca-certificates.crt",
        "/etc/ssl/cert.pem",
        "/etc/pki/tls/certs/ca-bundle.crt",
        "/usr/share/ca-certificates",
        "/dev/urandom",
    ] {
        if Path::new(path).exists() {
            rules = rules.add_rules(path_beneath_rules([path], AccessFs::from_read(ABI::V3)))?;
        }
    }
    if !bundle.is_empty() && Path::new(bundle).exists() {
        rules = rules.add_rules(path_beneath_rules([bundle], AccessFs::from_read(ABI::V3)))?;
    }
    if rules.restrict_self()?.ruleset != RulesetStatus::FullyEnforced {
        return Err("filesystem sandbox unavailable".into());
    }
    unsafe {
        let limit = libc::rlimit {
            rlim_cur: 2 * 1024 * 1024 * 1024,
            rlim_max: 2 * 1024 * 1024 * 1024,
        };
        if libc::setrlimit(libc::RLIMIT_AS, &limit) != 0 {
            return Err(std::io::Error::last_os_error().into());
        }
    }
    Ok(())
}
#[cfg(not(target_os = "linux"))]
fn sandbox(_: &Path, _: &Path, _: &str) -> Result<(), Box<dyn std::error::Error>> {
    Err("embedded compilation requires Linux Landlock isolation".into())
}
fn run(
    project: &Path,
    main: &str,
    cache: &Path,
    bundle: &str,
    offline: bool,
    status: &mut Status,
) -> Result<tectonic::io::memory::MemoryFileCollection, Box<dyn std::error::Error>> {
    sandbox(project, cache, bundle)?;
    let source = if bundle.is_empty() {
        "https://data1b.fullyjustified.net/tlextras-2022.0r0.tar".into()
    } else {
        bundle.into()
    };
    let pinned_default = bundle.is_empty();
    let mut bundle = tectonic_bundles::detect_bundle(source, offline, Some(cache.join("bundles")))?
        .ok_or("unsupported bundle")?;
    if pinned_default
        && bundle.get_digest()?.to_string()
            != "6ffe055852f8faf66c0acbe1a7fb27f87b869a90bad1204f3bf4d9683f597c7c"
    {
        return Err("default TeX bundle digest differs from pinned distribution".into());
    }
    let mut builder = ProcessingSessionBuilder::new_with_security(SecuritySettings::new(
        SecurityStance::DisableInsecures,
    ));
    let input = std::fs::read(project.join(main))?;
    let input_name = Path::new(main)
        .file_name()
        .ok_or("missing main file name")?
        .to_string_lossy();
    builder
        .primary_input_buffer(&input)
        .tex_input_name(&input_name)
        .filesystem_root(project)
        .do_not_write_output_files()
        .format_cache_path(cache.join("formats"))
        .bundle(bundle)
        .format_name("latex")
        .output_format(OutputFormat::Pdf)
        .keep_logs(true)
        .synctex(true)
        .shell_escape_disabled()
        .build_date(UNIX_EPOCH);
    let mut session = builder.create(status)?;
    if let Err(e) = session.run(status) {
        status.report(MessageKind::Error, format_args!("{e}"), None);
    }
    Ok(session.into_file_data())
}
pub(crate) fn run_worker(args: Vec<String>) {
    if args.len() != 6 {
        std::process::exit(2);
    }
    let project = PathBuf::from(&args[1]);
    let main = &args[2];
    let cache = PathBuf::from(&args[3]);
    // A minimal fontconfig file avoids consulting host font inventories.
    let font_config = project.join(".fontconfig.conf");
    let _ = std::fs::write(
        &font_config,
        "<?xml version=\"1.0\"?><!DOCTYPE fontconfig SYSTEM \"fonts.dtd\"><fontconfig></fontconfig>",
    );
    unsafe {
        std::env::set_var("FONTCONFIG_FILE", &font_config);
    }
    let mut status = Status::default();
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        run(
            &project,
            main,
            &cache,
            &args[4],
            args[5] == "1",
            &mut status,
        )
    }))
    .unwrap_or_else(|_| {
        Err("embedded engine initialization failed; check bundle/cache configuration".into())
    });
    let mut files = match result {
        Ok(files) => files,
        Err(e) => {
            status.report(MessageKind::Error, format_args!("{e}"), None);
            Default::default()
        }
    };
    let stem = Path::new(main)
        .file_stem()
        .unwrap_or_default()
        .to_string_lossy();
    if let Some(log) = files.remove(&format!("{stem}.log")) {
        append_limited(
            &mut status.log,
            &String::from_utf8_lossy(&log.data[..log.data.len().min(1024 * 1024)]),
            1024 * 1024,
        );
    }
    let mut pdf = files.remove(&format!("{stem}.pdf")).map(|f| f.data);
    if pdf
        .as_ref()
        .is_some_and(|data| data.len() > 16 * 1024 * 1024)
    {
        pdf = None;
        status.report(
            MessageKind::Error,
            format_args!("PDF exceeds 16 MiB artifact limit"),
            None,
        );
    }
    let success = pdf.is_some() && !status.had_error;
    for diagnostic in &mut status.diagnostics {
        let mut fields = diagnostic.message.splitn(3, ':');
        if let (Some(name), Some(line), Some(_)) = (fields.next(), fields.next(), fields.next()) {
            if let Ok(line) = line.parse::<u32>() {
                if crate::validate_path(name).is_ok() {
                    let file = if project.join(name).is_file() {
                        Some(name.to_owned())
                    } else if project.join(format!("{name}.tex")).is_file() {
                        Some(format!("{name}.tex"))
                    } else if Path::new(main).file_name().is_some_and(|n| n == name) {
                        Some(main.to_owned())
                    } else {
                        None
                    };
                    if file.is_some() {
                        diagnostic.file = file;
                        diagnostic.line = Some(line);
                    }
                }
            }
        }
    }
    let output = CompileOutput {
        success,
        pdf: if success { pdf } else { None },
        log: status.log,
        diagnostics: status.diagnostics,
        synctex: files
            .remove(&format!("{stem}.synctex.gz"))
            .filter(|f| f.data.len() <= 4 * 1024 * 1024)
            .map(|f| f.data),
    };
    if let Ok(bytes) = serde_json::to_vec(&output) {
        let _ = std::fs::write(project.join(".compile-result.json"), bytes);
    }
}
