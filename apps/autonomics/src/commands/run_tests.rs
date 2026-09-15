use super::*;

fn test_args(prompt: Option<&str>, list_sessions: bool) -> RunArgs {
    RunArgs {
        prompt: prompt.map(str::to_string),
        list_sessions,
        json: false,
        output_last_message: None,
        profile: None,
        agent_config: None,
        no_memory: false,
        model: None,
        timeout: None,
        session: None,
        ephemeral: false,
        backend: None,
        workspace: None,
        data_mount: Vec::new(),
        mount_manifest: None,
        resume_workspace: false,
        keep_state: false,
        manifest: None,
    }
}

#[test]
fn ephemeral_cannot_resume_a_discarded_state_dir() {
    let mut args = test_args(Some("prompt"), false);
    args.ephemeral = true;
    args.session = Some(uuid::Uuid::nil());
    assert!(validate_run_args(&args).is_err());
}

#[test]
fn list_sessions_rejects_run_options() {
    let mut args = test_args(None, true);
    args.timeout = Some(1);
    assert!(validate_run_args(&args).is_err());
}

#[test]
fn no_memory_overrides_both_memory_settings() {
    let mut args = test_args(Some("prompt"), false);
    args.no_memory = true;
    let runtime = parse_agent_runtime(&args).unwrap();
    assert_eq!(runtime.use_memory, Some(false));
    assert_eq!(runtime.generate_memory, Some(false));
}

#[test]
fn agent_config_json_accepts_partial_overrides() {
    let mut args = test_args(Some("prompt"), false);
    args.agent_config = Some(r#"{"generate_memory":false}"#.into());
    let runtime = parse_agent_runtime(&args).unwrap();
    assert_eq!(runtime.use_memory, None);
    assert_eq!(runtime.generate_memory, Some(false));
}

#[test]
fn agent_config_rejects_unknown_fields() {
    let mut args = test_args(Some("prompt"), false);
    args.agent_config = Some(r#"{"typo":true}"#.into());
    assert!(parse_agent_runtime(&args).is_err());
}

#[test]
fn ephemeral_gateway_backend_is_rejected_until_implemented() {
    let mut args = test_args(Some("prompt"), false);
    args.ephemeral = true;
    args.backend = Some(RunBackend::Gateway);
    assert_eq!(
        validate_run_args(&args).unwrap_err(),
        "--backend gateway is not implemented for --ephemeral yet; use in-process"
    );
}

#[test]
fn mount_options_require_ephemeral() {
    let mut args = test_args(Some("prompt"), false);
    args.data_mount.push("/absolute/data=/data".to_owned());
    assert_eq!(
        validate_run_args(&args).unwrap_err(),
        "mount and ephemeral-state options require --ephemeral"
    );
}

#[test]
fn mount_arguments_use_source_equals_virtual_path() {
    let mount = parse_mount_argument("/absolute/data=/data", true).unwrap();
    assert_eq!(mount.source, std::path::Path::new("/absolute/data"));
    assert_eq!(mount.target, "/data");
    assert!(mount.read_only);
    assert!(parse_mount_argument("data=/data", true).is_err());
    assert!(parse_mount_argument("/absolute/data=data", true).is_err());
}
