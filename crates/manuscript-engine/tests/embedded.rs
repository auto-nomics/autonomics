use manuscript_engine::{EngineConfig, EngineError, validate_path};
use std::{collections::BTreeMap, path::PathBuf, time::Duration};

#[test]
fn rejects_escaping_project_paths() {
    for path in [
        "",
        "/etc/passwd",
        "../secret",
        "part/../../secret",
        "a\\b",
        ".compile-result.json",
    ] {
        assert!(validate_path(path).is_err(), "{path}");
    }
    assert!(validate_path("sections/introduction.tex").is_ok());
}

/// Run explicitly when provisioning/verifying the pinned TeX bundle. Afterwards it runs offline.
#[tokio::test]
#[ignore = "downloads pinned TeX bundle resources on first run"]
async fn embedded_multifile_bibtex_security_and_timeout() {
    let mut config = EngineConfig::from_env();
    config.worker = PathBuf::from(env!("CARGO_BIN_EXE_autonomics-manuscript-worker"));
    config.timeout = Duration::from_secs(300);
    let files = BTreeMap::from([
        ("main.tex".into(), br"\documentclass{article}\usepackage{graphicx}\begin{document}\input{sections/intro}\includegraphics{figures/figure.png}\bibliographystyle{plain}\bibliography{references}\end{document}".to_vec()),
        ("sections/intro.tex".into(), br"An embedded kernel citation~\cite{knuth1984}.".to_vec()),
        ("references.bib".into(), br"@book{knuth1984,title={The TeXbook},author={Knuth, Donald E.},year={1984},publisher={Addison-Wesley}}".to_vec()),
        ("figures/figure.png".into(), include_bytes!("fixtures/figure.png").to_vec()),
    ]);
    let result = config.compile(files, "main.tex").await.unwrap();
    assert!(result.success, "{}", result.log);
    assert!(result.pdf.unwrap().starts_with(b"%PDF-"));
    assert!(
        !result
            .log
            .contains("Citation `knuth1984' on page 1 undefined")
    );
    let broken = config.compile(BTreeMap::from([
        ("main.tex".into(), br"\documentclass{article}\begin{document}\input{sections/broken}\end{document}".to_vec()),
        ("sections/broken.tex".into(), b"First line.\n\\unknowncommand\n".to_vec()),
    ]), "main.tex").await.unwrap();
    assert!(!broken.success);
    assert!(
        broken
            .diagnostics
            .iter()
            .any(|d| d.file.as_deref() == Some("sections/broken.tex") && d.line == Some(2)),
        "{:?}",
        broken.diagnostics
    );
    // Deny absolute reads outside the fresh project even if the path exists.
    let secret = tempfile::NamedTempFile::new().unwrap();
    std::fs::write(secret.path(), "PRIVATE-MANUSCRIPT-TEST-SECRET").unwrap();
    let tex = format!(
        r"\documentclass{{article}}\begin{{document}}\input{{{}}}\end{{document}}",
        secret.path().display()
    );
    let denied = config
        .compile(
            BTreeMap::from([("main.tex".into(), tex.into_bytes())]),
            "main.tex",
        )
        .await
        .unwrap();
    assert!(!denied.success);
    assert!(!denied.log.contains("PRIVATE-MANUSCRIPT-TEST-SECRET"));
    // No external shell command may run. The hostile command would create a marker.
    let marker = secret.path().with_extension("shell-marker");
    let tex = format!(
        r"\documentclass{{article}}\begin{{document}}\immediate\write18{{touch {}}}Safe.\end{{document}}",
        marker.display()
    );
    let _ = config
        .compile(
            BTreeMap::from([("main.tex".into(), tex.into_bytes())]),
            "main.tex",
        )
        .await
        .unwrap();
    assert!(!marker.exists());
    config.timeout = Duration::from_millis(50);
    let timeout = config
        .compile(
            BTreeMap::from([(
                "main.tex".into(),
                br"\documentclass{article}\begin{document}\loop\iftrue\repeat\end{document}"
                    .to_vec(),
            )]),
            "main.tex",
        )
        .await;
    assert!(matches!(timeout, Err(EngineError::Timeout)));
}
