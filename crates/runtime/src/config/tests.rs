use std::path::PathBuf;

use bib_base::BibHttpOptions;
use container_runtime::ImageReference;

use super::defaults::*;
use super::runtime::default_kms_db_path;
use super::*;

static BIB_HTTP_ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
static DATA_DIR_ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

#[test]
fn defaults() {
    // Guard against env-var pollution from parallel tests like
    // `env_data_dir_override` that set AUTONOMICS_DATA_DIR.
    let _guard = DATA_DIR_ENV_LOCK.lock().unwrap();
    let saved_data_dir = std::env::var_os(ENV_DATA_DIR);
    let saved_state_dir = std::env::var_os(ENV_STATE_DIR);
    // SAFETY: single-threaded within this test fn.
    unsafe {
        std::env::remove_var(ENV_DATA_DIR);
        std::env::remove_var(ENV_STATE_DIR);
    }

    let cfg = RuntimeConfig::default();
    assert_eq!(cfg.name, "default");
    // data_dir resolves to $HOME/.autonomics/data when HOME is set.
    let expected_data_dir = std::env::var_os("HOME")
        .map(|h| PathBuf::from(h).join(DEFAULT_DATA_DIR))
        .unwrap_or_else(|| PathBuf::from(DEFAULT_DATA_DIR));
    assert_eq!(cfg.data_dir, expected_data_dir);

    // state_dir resolves to $HOME/.autonomics when HOME is set.
    let expected_state_dir = std::env::var_os("HOME")
        .map(|h| PathBuf::from(h).join(DEFAULT_STATE_DIR))
        .unwrap_or_else(|| PathBuf::from(DEFAULT_STATE_DIR));
    assert_eq!(cfg.state_dir, expected_state_dir);
    assert_eq!(
        cfg.dag_history_db,
        expected_state_dir.join(DEFAULT_DAG_HISTORY_DB)
    );
    assert_eq!(cfg.bib_db_path, expected_state_dir.join(DEFAULT_BIB_DB));
    assert_eq!(cfg.app_db_path, expected_state_dir.join(DEFAULT_APP_DB));
    assert_eq!(cfg.kms_db_path, expected_state_dir.join(DEFAULT_KMS_DB));
    assert_eq!(cfg.browser_download_dir, None);
    assert!(cfg.enable_dag_history);
    assert!(cfg.enable_bibliography);
    assert!(cfg.enable_opengwas);
    assert!(cfg.enable_opentargets);
    assert!(cfg.enable_gwascatalog);
    assert!(cfg.enable_chembl);
    assert!(cfg.enable_rcsb);
    assert!(cfg.enable_string);
    assert!(cfg.use_memory);
    assert!(cfg.generate_memory);
    assert!(!cfg.enable_kms);
    // Remote Ollama embeddings are on by default.
    assert!(cfg.embedding.enabled);

    // Restore env vars.
    // SAFETY: single-threaded within this test fn.
    unsafe {
        if let Some(v) = saved_data_dir {
            std::env::set_var(ENV_DATA_DIR, v);
        }
        if let Some(v) = saved_state_dir {
            std::env::set_var(ENV_STATE_DIR, v);
        }
    }
}

#[test]
fn builder_overrides() {
    let cfg = RuntimeConfig::builder()
        .name("test-agent")
        .data_dir("/tmp/test-data")
        .state_dir("/tmp/test-state")
        .dag_history_db("/tmp/custom-history.db")
        .bib_db_path("/tmp/custom-bib.db")
        .app_db_path("/tmp/custom-app.db")
        .kms_db_path("/tmp/custom-kms.db")
        .browser_download_dir("/tmp/browser-downloads")
        .opengwas_token("secret-token")
        .agent_identity("Custom agent")
        .system_prompt(Some("Custom prompt".to_string()))
        .enable_dag_history(false)
        .enable_opengwas(false)
        .enable_rcsb(false)
        .use_memory(false)
        .generate_memory(true)
        .enable_kms(true)
        .build();

    assert_eq!(cfg.name, "test-agent");
    assert_eq!(cfg.data_dir, PathBuf::from("/tmp/test-data"));
    assert_eq!(cfg.state_dir, PathBuf::from("/tmp/test-state"));
    assert_eq!(cfg.dag_history_db, PathBuf::from("/tmp/custom-history.db"));
    assert_eq!(cfg.bib_db_path, PathBuf::from("/tmp/custom-bib.db"));
    assert_eq!(cfg.app_db_path, PathBuf::from("/tmp/custom-app.db"));
    assert_eq!(cfg.kms_db_path, PathBuf::from("/tmp/custom-kms.db"));
    assert_eq!(
        cfg.browser_download_dir,
        Some(PathBuf::from("/tmp/browser-downloads"))
    );
    assert_eq!(cfg.opengwas_token.as_deref(), Some("secret-token"));
    assert_eq!(cfg.agent_identity, "Custom agent");
    assert_eq!(cfg.system_prompt.as_deref(), Some("Custom prompt"));
    assert!(!cfg.enable_dag_history);
    assert!(!cfg.enable_opengwas);
    // Flags not touched → still true
    assert!(cfg.enable_bibliography);
    assert!(cfg.enable_opentargets);
    assert!(cfg.enable_gwascatalog);
    assert!(cfg.enable_chembl);
    assert!(!cfg.enable_rcsb);
    assert!(cfg.enable_string);
    assert!(!cfg.use_memory);
    assert!(cfg.generate_memory);
    assert!(cfg.enable_kms);
}

#[test]
fn system_prompt_fallback() {
    let cfg = RuntimeConfig::default();
    assert!(cfg.system_prompt.is_none());
    assert!(cfg.system_prompt_or_default().contains("Core Competencies"));

    let cfg = RuntimeConfig::builder()
        .system_prompt(Some("short".to_string()))
        .build();
    assert_eq!(cfg.system_prompt_or_default(), "short");

    let cfg = RuntimeConfig::builder().build();
    assert!(
        cfg.system_prompt_or_default()
            .contains("Protein Association (STRING)")
    );

    let cfg = RuntimeConfig::builder().enable_string(false).build();
    assert!(
        !cfg.system_prompt_or_default()
            .contains("Protein Association (STRING)")
    );
}

#[test]
fn system_prompt_dynamic_sections() {
    // All sections enabled (default config).
    let cfg = RuntimeConfig::default();
    let prompt = build_system_prompt(&cfg);
    assert!(prompt.contains("Literature & Evidence"));
    assert!(prompt.contains("OpenGWAS API"));
    assert!(prompt.contains("Open Targets Platform"));
    assert!(prompt.contains("GWAS Catalog (EBI)"));
    assert!(prompt.contains("Structural Biology (RCSB PDB)"));
    assert!(prompt.contains("Protein Association (STRING)"));
    assert!(prompt.contains("DAG Version Control"));
    // Minimal config — no external service tools.
    let cfg = RuntimeConfig::builder()
        .enable_bibliography(false)
        .enable_opengwas(false)
        .enable_opentargets(false)
        .enable_gwascatalog(false)
        .enable_string(false)
        .enable_chembl(false)
        .enable_rcsb(false)
        .enable_dag_history(false)
        .build();
    let prompt = build_system_prompt(&cfg);
    assert!(!prompt.contains("Literature & Evidence"));
    assert!(!prompt.contains("OpenGWAS API"));
    assert!(!prompt.contains("Open Targets Platform"));
    assert!(!prompt.contains("GWAS Catalog (EBI)"));
    assert!(!prompt.contains("Drug & Bioactivity Data (ChEMBL)"));
    assert!(!prompt.contains("Structural Biology (RCSB PDB)"));
    assert!(!prompt.contains("Protein Association (STRING)"));
    assert!(!prompt.contains("DAG Version Control"));
    // These are always present:
    assert!(prompt.contains("Core Competencies"));
    assert!(prompt.contains("Data Pipeline (DAG Engine)"));
    assert!(prompt.contains("SQL Conventions"));
    assert!(prompt.contains("## Guidelines"));
}

#[test]
fn system_prompt_separates_researcher_and_developer_handoff() {
    use agentik_core::AgentKind;

    let researcher_prompt = build_system_prompt(&AgentKind::Researcher);
    assert!(researcher_prompt.contains("Researcher / Developer Collaboration"));
    assert!(researcher_prompt.contains("delegate_to"));
    assert!(researcher_prompt.contains("profile_segment=\"developer\""));
    assert!(researcher_prompt.contains("does not execute your research dataset"));
    assert!(researcher_prompt.contains("Do not hand over production or"));
    assert!(researcher_prompt.contains("do not attempt to patch the plugin"));
    assert!(!researcher_prompt.contains("Plugin Development"));
    assert!(!researcher_prompt.contains("Developer Handoff"));

    let developer_prompt = build_system_prompt(&AgentKind::Developer);
    assert!(developer_prompt.contains("Plugin Development"));
    assert!(developer_prompt.contains("Developer Handoff"));
    assert!(developer_prompt.contains("Maintain the execution boundary"));
    assert!(developer_prompt.contains("explicitly reject that task"));
    assert!(developer_prompt.contains("synthetic"));
    assert!(developer_prompt.contains("full `plugin/node` address"));
    assert!(developer_prompt.contains("validation evidence"));
    assert!(!developer_prompt.contains("Researcher / Developer Collaboration"));
    // Neither researcher nor developer should advertise the browser
    // tool group — only the Browser kind should.
    assert!(!researcher_prompt.contains("Web Browsing"));
    assert!(!developer_prompt.contains("Web Browsing"));
}

#[test]
fn system_prompt_advertises_central_downloads_for_researcher_only() {
    use agentik_core::AgentKind;

    assert!(build_system_prompt(&AgentKind::Researcher).contains("Central Resource Downloads"));
    for kind in [AgentKind::Developer, AgentKind::Browser] {
        assert!(!build_system_prompt(&kind).contains("Central Resource Downloads"));
    }
}

#[test]
fn system_prompt_advertises_literature_for_researcher_and_curator_only() {
    use agentik_core::AgentKind;

    for kind in [AgentKind::Researcher, AgentKind::Curator] {
        let prompt = build_system_prompt(&kind);
        assert!(
            prompt.contains("### Literature & Evidence"),
            "{} must advertise the literature group",
            kind.name()
        );
        assert!(
            prompt.contains("literature_search"),
            "{} must name the unified retrieval tool",
            kind.name()
        );
    }
    for kind in [AgentKind::Developer, AgentKind::Browser] {
        let prompt = build_system_prompt(&kind);
        assert!(
            !prompt.contains("### Literature & Evidence"),
            "{} must not advertise the literature group",
            kind.name()
        );
    }
}

#[test]
fn system_prompt_advertises_wiki_read_for_every_kind_and_write_for_curator_only() {
    use agentik_core::AgentKind;

    for kind in AgentKind::ALL {
        let prompt = build_system_prompt(&kind);
        assert!(
            prompt.contains("### Knowledge Base (Read)"),
            "{} must advertise wiki read tools",
            kind.name()
        );
        assert!(
            prompt.contains("wiki_search"),
            "{} must name the retrieval tool",
            kind.name()
        );
        if kind == AgentKind::Curator {
            continue;
        }
        assert!(
            !prompt.contains("### Knowledge Base (Curation)"),
            "{} must not advertise wiki write tools",
            kind.name()
        );
        assert!(
            !prompt.contains("wiki_create"),
            "{} must not name write tools",
            kind.name()
        );
    }

    let curator_prompt = build_system_prompt(&AgentKind::Curator);
    assert!(curator_prompt.contains("### Knowledge Base (Read)"));
    assert!(curator_prompt.contains("### Knowledge Base (Curation)"));
    assert!(curator_prompt.contains("wiki_create"));
    assert!(curator_prompt.contains("wiki_patch"));
    assert!(curator_prompt.contains("wiki_restore_version"));
}

#[test]
fn system_prompt_advertises_browser_for_browser_kind_only() {
    use agentik_core::AgentKind;

    let browser_prompt = build_system_prompt(&AgentKind::Browser);
    assert!(browser_prompt.contains("Web Browsing"));
    assert!(browser_prompt.contains("browser_navigate"));
    assert!(browser_prompt.contains("browser_screenshot"));
    assert!(browser_prompt.contains("browser_evaluate"));
    assert!(browser_prompt.contains("browser_download"));
    assert!(browser_prompt.contains("browser_wait_for_download"));
    assert!(browser_prompt.contains("/browser-downloads"));
    assert!(browser_prompt.contains("Collaboration Boundary"));
    // Browser agents are not researchers and not plugin developers;
    // those sections must be absent so the prompt doesn't waste
    // context advertising tools they don't have.
    assert!(!browser_prompt.contains("Literature & Evidence"));
    assert!(!browser_prompt.contains("Plugin Development"));
    assert!(!browser_prompt.contains("Researcher / Developer Collaboration"));
}

#[test]
fn system_prompt_requires_dedicated_node_priority() {
    let prompt = RuntimeConfig::default().system_prompt_or_default();

    assert!(prompt.contains("**Prefer dedicated nodes**"));
    // The visualization bullet must keep pinning computation
    // upstream: it is a terminal sink, never a compute node.
    assert!(prompt.contains("**Visualization**"));
    assert!(prompt.contains("terminal sink for plot-ready data"));
    assert!(!prompt.contains("python_script"));
    assert!(!prompt.contains("r_script"));
    assert!(!prompt.contains("container_command"));
}

#[test]
fn skill_evolution_defaults_and_overrides() {
    // Defaults: service on, automatic installs, 6h sweep.
    let cfg = RuntimeConfig::default();
    assert!(cfg.enable_skill_evolution);
    assert_eq!(cfg.skill_evolution_interval_secs, 6 * 60 * 60);

    // Builder overrides flow through resolve.
    let cfg = RuntimeConfig::builder()
        .enable_skill_evolution(false)
        .skill_evolution_interval_secs(600)
        .build();
    assert!(!cfg.enable_skill_evolution);
    assert_eq!(cfg.skill_evolution_interval_secs, 600);
}

#[test]
fn plugin_rsi_defaults_builder_and_backward_compatibility() {
    let config = RuntimeConfig::default();
    assert!(config.plugin_rsi.environments.list().len() >= 32);
    for (id, environment) in config.plugin_rsi.environments.list() {
        ImageReference::parse(&environment.reference)
            .unwrap_or_else(|error| panic!("default environment `{id}` is invalid: {error}"));
    }
    assert_eq!(
        config
            .plugin_rsi
            .environments
            .get("alpine")
            .map(|environment| environment.reference.as_str()),
        Some(
            "docker.io/library/alpine@sha256:ce64758a109eb420d874a118f87920e625e12d3634e03b4a5573fd9f6e5d3507"
        )
    );
    assert!(config.plugin_rsi.publisher.enabled);
    assert_eq!(config.plugin_rsi.publisher.owner, "auto-nomics");

    let mut environments = plugin_rsi::EnvironmentCatalog::default();
    environments.insert(
            "alpine",
            plugin_rsi::Environment {
                reference: "docker.io/library/alpine@sha256:0123456789012345678901234567890123456789012345678901234567890123".into(),
                interpreters: vec!["sh".into()],
            },
        );
    let config = RuntimeConfig::builder()
        .plugin_rsi(PluginRsiConfig {
            environments,
            publisher: plugin_rsi::GhPublisherConfig {
                enabled: false,
                owner: "example".into(),
                ..Default::default()
            },
            ..Default::default()
        })
        .build();
    assert!(config.plugin_rsi.environments.get("alpine").is_some());
    assert!(!config.plugin_rsi.publisher.enabled);
    assert_eq!(config.plugin_rsi.publisher.owner, "example");

    let mut legacy = serde_json::to_value(RuntimeConfig::default()).unwrap();
    legacy.as_object_mut().unwrap().remove("plugin_rsi");
    let config: RuntimeConfig = serde_json::from_value(legacy).unwrap();
    assert!(config.plugin_rsi.environments.get("alpine").is_some());
    assert!(config.plugin_rsi.publisher.enabled);
}

#[test]
fn serde_tolerates_missing_kms_db_path() {
    // A serialized config from before the dedicated knowledge database
    // must still deserialize (the field falls back to its default).
    let mut legacy = serde_json::to_value(RuntimeConfig::default()).unwrap();
    legacy.as_object_mut().unwrap().remove("kms_db_path");
    let config: RuntimeConfig = serde_json::from_value(legacy).unwrap();
    assert_eq!(config.kms_db_path, default_kms_db_path());
}

#[test]
fn env_kms_db_override() {
    let _guard = DATA_DIR_ENV_LOCK.lock().unwrap();
    // SAFETY: guarded by DATA_DIR_ENV_LOCK for tests that assert defaults.
    unsafe {
        std::env::set_var(ENV_KMS_DB, "/tmp/env-kms.db");
    }
    let cfg = RuntimeConfig::default();
    assert_eq!(cfg.kms_db_path, PathBuf::from("/tmp/env-kms.db"));
    unsafe {
        std::env::remove_var(ENV_KMS_DB);
    }
}

#[test]
fn env_data_dir_override() {
    let _guard = DATA_DIR_ENV_LOCK.lock().unwrap();
    // SAFETY: guarded by DATA_DIR_ENV_LOCK for tests that assert defaults.
    unsafe {
        std::env::set_var(ENV_DATA_DIR, "/tmp/env-data");
    }
    let cfg = RuntimeConfig::default();
    assert_eq!(cfg.data_dir, PathBuf::from("/tmp/env-data"));
    unsafe {
        std::env::remove_var(ENV_DATA_DIR);
    }
}

#[test]
fn builder_takes_precedence_over_env() {
    let _guard = DATA_DIR_ENV_LOCK.lock().unwrap();
    // SAFETY: guarded by DATA_DIR_ENV_LOCK for tests that assert defaults.
    unsafe {
        std::env::set_var(ENV_DATA_DIR, "/tmp/env-data");
    }
    let cfg = RuntimeConfig::builder()
        .data_dir("/tmp/explicit-data")
        .build();
    assert_eq!(cfg.data_dir, PathBuf::from("/tmp/explicit-data"));
    unsafe {
        std::env::remove_var(ENV_DATA_DIR);
    }
}

#[test]
fn bib_http_defaults_to_all_none() {
    let _guard = BIB_HTTP_ENV_LOCK.lock().unwrap();
    let cfg = RuntimeConfig::default();
    assert!(cfg.bib_http.user_agent.is_none());
    assert!(cfg.bib_http.connect_timeout.is_none());
    assert!(cfg.bib_http.request_timeout.is_none());
    assert!(cfg.bib_http.proxy_url.is_none());
    assert!(cfg.bib_http.accept_invalid_certs.is_none());
}

#[test]
fn downloader_builder_overrides_deny_by_default() {
    let mut downloader = resource_downloader::DownloaderConfig::default();
    downloader.policy = resource_downloader::DownloadPolicy::allow_https("example.com", "/data/");
    let cfg = RuntimeConfig::builder()
        .downloader(downloader.clone())
        .build();
    assert_eq!(cfg.downloader, downloader);
    assert!(RuntimeConfig::default().downloader.policy.allow.is_empty());
}

#[test]
fn bib_http_builder_overrides() {
    let opts = BibHttpOptions {
        user_agent: Some("autonomics-test/0.1".into()),
        connect_timeout: Some(std::time::Duration::from_secs(3)),
        request_timeout: Some(std::time::Duration::from_secs(20)),
        proxy_url: Some("http://proxy.test:3128".into()),
        accept_invalid_certs: Some(false),
    };
    let cfg = RuntimeConfig::builder().bib_http(opts.clone()).build();
    assert_eq!(cfg.bib_http, opts);
}

#[test]
fn bib_http_env_user_agent_override() {
    let _guard = BIB_HTTP_ENV_LOCK.lock().unwrap();
    unsafe {
        std::env::set_var(ENV_HTTP_USER_AGENT, "from-env");
    }
    let cfg = RuntimeConfig::default();
    assert_eq!(cfg.bib_http.user_agent.as_deref(), Some("from-env"));
    unsafe {
        std::env::remove_var(ENV_HTTP_USER_AGENT);
    }
}
