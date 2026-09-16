use super::*;

fn test_args(prompt: Option<&str>, list_sessions: bool) -> RunArgs {
    RunArgs {
        prompt: prompt.map(str::to_string),
        name: "test_agent".to_string(),
        list_sessions,
        json: false,
        output_last_message: None,
        profile: None,
        agent_config: None,
        no_memory: false,
        model: None,
        timeout: None,
        session: None,
        manifest: None,
    }
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
