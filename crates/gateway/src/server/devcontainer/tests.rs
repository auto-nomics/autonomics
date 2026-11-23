use super::*;

async fn debug_message(
    server: &mut super::super::devlsp::LanguageServer,
    offset: &mut u64,
    pending: &mut Vec<Value>,
    predicate: impl Fn(&Value) -> bool,
) -> Value {
    timeout(Duration::from_secs(30), async {
        loop {
            if let Some(index) = pending.iter().position(&predicate) {
                return pending.remove(index);
            }
            let (messages, next, truncated, exited) = server.poll(*offset).unwrap();
            assert!(
                !truncated && !exited,
                "debug adapter ended before expected message: {pending:?}"
            );
            *offset = next;
            pending.extend(messages);
            sleep(Duration::from_millis(30)).await;
        }
    })
    .await
    .expect("debug adapter response")
}

#[test]
fn file_paths_stay_inside_workspace() {
    for path in [
        "",
        "../secret",
        "/etc/passwd",
        ".git/config",
        "a/.git/config",
        "a//b",
        "a/./b",
        "a/",
        "a\0b",
    ] {
        assert!(validate_file_path(path).is_err(), "{path:?}");
    }
    for path in [
        "manifest.toml",
        "src/main.py",
        ".rsi/reports/test.json",
        "目录/a b.R",
    ] {
        assert!(validate_file_path(path).is_ok(), "{path:?}");
    }
}

#[test]
fn directory_move_cannot_target_itself_or_descendants() {
    assert!(validate_directory_move("src", "src").is_err());
    assert!(validate_directory_move("src", "src/nested").is_err());
    assert!(validate_directory_move("src", "src/../elsewhere").is_err());
    assert!(validate_directory_move("src", "src2").is_ok());
}

#[tokio::test]
async fn file_access_requires_a_session() {
    let manager = DevContainers::default();
    let result = manager
        .exec(&PodmanConfig::default(), "missing", "exit 0", &[], &[])
        .await;
    assert!(matches!(
        result,
        Err(GatewayError::Status(StatusCode::SERVICE_UNAVAILABLE, _))
    ));
}

#[cfg(unix)]
#[tokio::test]
async fn closing_session_stays_reserved_until_container_exits() {
    let dir = tempfile::tempdir().unwrap();
    let marker = dir.path().join("closing");
    let child = tokio::process::Command::new("/bin/sh")
        .args(["-c", "cat >/dev/null; : > \"$1\"; sleep 1", "test"])
        .arg(&marker)
        .stdin(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .unwrap();
    let manager = Arc::new(DevContainers::default());
    let slot = manager.slot("project", true).await.unwrap();
    *slot.lock().await = Some(Session {
        name: "container".into(),
        settings: super::super::devcontainer_config::DevConfig::load(dir.path(), "image")
            .unwrap_or_else(|_| panic!("default config")),
        workspace: dir.path().to_path_buf(),
        remote_env: BTreeMap::new(),
        child,
        terminal: None,
        language_servers: HashMap::new(),
        vscode: None,
    });
    let closing = {
        let manager = manager.clone();
        tokio::spawn(async move { manager.close("project").await })
    };
    timeout(Duration::from_secs(5), async {
        while !marker.exists() {
            sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    assert!(
        timeout(Duration::from_millis(150), manager.active("project"))
            .await
            .is_err()
    );
    assert!(closing.await.unwrap());
    assert!(manager.active("project").await.is_err());
}

#[cfg(unix)]
#[test]
fn vscode_data_is_private_separate_and_rejects_symlinks() {
    use std::os::unix::fs::{PermissionsExt, symlink};
    let dir = tempfile::tempdir().unwrap();
    let mut config = PodmanConfig::default();
    config.workspace_root = dir.path().join("workspace");
    let alpha = vscode_data_dir(&config, "alpha").unwrap_or_else(|_| panic!("alpha data"));
    let beta = vscode_data_dir(&config, "beta").unwrap_or_else(|_| panic!("beta data"));
    assert_ne!(alpha, beta);
    assert_eq!(
        vscode_data_dir(&config, "alpha").unwrap_or_else(|_| panic!("reuse data")),
        alpha
    );
    assert_eq!(
        std::fs::metadata(alpha.parent().unwrap())
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o700
    );
    assert!(vscode_data_dir(&config, "../escape").is_err());
    symlink(dir.path(), alpha.parent().unwrap().join("linked")).unwrap();
    assert!(vscode_data_dir(&config, "linked").is_err());
    std::fs::set_permissions(
        alpha.parent().unwrap(),
        std::fs::Permissions::from_mode(0o755),
    )
    .unwrap();
    assert!(vscode_data_dir(&config, "alpha").is_err());
}

#[cfg(unix)]
#[tokio::test]
#[ignore = "requires a local test image, Podman state and AUTONOMICS_VSCODE_SERVER_DIR"]
async fn real_container_starts_vscode_in_project_session() {
    use std::os::unix::fs::PermissionsExt;
    let state = std::env::var("AUTONOMICS_DEVCONTAINER_TEST_STATE").expect("test state");
    let image = std::env::var("AUTONOMICS_DEVCONTAINER_TEST_IMAGE").expect("test image");
    assert!(std::env::var_os("AUTONOMICS_VSCODE_SERVER_DIR").is_some());
    let mut config =
        container_runtime::ContainerExecutionInfra::for_operator_state_dir(Path::new(&state))
            .expect("container configuration")
            .config;
    let runtime_dir = tempfile::tempdir().unwrap();
    config.workspace_root = runtime_dir.path().join("runtime");
    let workspace = tempfile::tempdir().unwrap();
    std::fs::write(
        workspace.path().join("hello.py"),
        "print('container workbench')\n",
    )
    .unwrap();
    std::fs::write(
        workspace.path().join("second.py"),
        "print('bridge opened file')\n",
    )
    .unwrap();
    let manager = DevContainers::default();
    manager
        .connect(&config, "vscode-test", &image, workspace.path())
        .await
        .unwrap_or_else(|_| panic!("connect VS Code container"));
    let (socket_dir, workbench_url) = {
        let slot = manager
            .active("vscode-test")
            .await
            .unwrap_or_else(|_| panic!("active VS Code container"));
        let vscode = slot.as_ref().unwrap().vscode.as_ref().unwrap();
        (
            vscode._socket_dir.path().to_owned(),
            vscode.listener.url.clone(),
        )
    };
    assert_eq!(
        std::fs::metadata(&socket_dir).unwrap().permissions().mode() & 0o777,
        0o700
    );
    let stream = manager
        .vscode_connection("vscode-test")
        .await
        .unwrap_or_else(|_| panic!("connect to VS Code socket"));
    let request = axum::http::Request::builder()
        .uri("/api/v1/plugin-projects/vscode-test/vscode/?folder=/work")
        .header("host", "localhost")
        .body(axum::body::Body::empty())
        .unwrap();
    let response = timeout(
        Duration::from_secs(30),
        super::super::devvscode::forward_with_prefix(stream, request, None),
    )
    .await
    .unwrap()
    .unwrap_or_else(|_| panic!("proxy VS Code response"));
    assert!(
        response.status() == StatusCode::OK || response.status() == StatusCode::FOUND,
        "{}",
        response.status()
    );
    let client = reqwest::Client::builder().no_proxy().build().unwrap();
    let response = client.get(&workbench_url).send().await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response.headers().get("referrer-policy").unwrap(),
        "no-referrer"
    );
    let html = response.text().await.unwrap();
    assert!(html.contains("workbench"));
    let origin = workbench_url
        .split('/')
        .take(3)
        .collect::<Vec<_>>()
        .join("/");
    assert_eq!(
        client
            .get(format!(
                "{origin}/wrong/api/v1/plugin-projects/vscode-test/vscode/"
            ))
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        client
            .get(&workbench_url)
            .header("host", "attacker.example")
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        client
            .get(&workbench_url)
            .header("connection", "Upgrade")
            .header("upgrade", "websocket")
            .header("origin", "http://attacker.example")
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::NOT_FOUND
    );
    if std::env::var_os("AUTONOMICS_DEVCONTAINER_TEST_BROWSER").is_some() {
        let parent_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let parent_url = format!("http://{}/", parent_listener.local_addr().unwrap());
        let parent_html = format!(
            r#"<iframe src="{workbench_url}" sandbox="allow-scripts allow-same-origin allow-forms allow-modals allow-popups allow-downloads allow-pointer-lock" style="width:1400px;height:900px"></iframe>"#
        );
        let parent_app = axum::Router::new().route(
            "/",
            axum::routing::get(move || {
                let html = parent_html.clone();
                async move { axum::response::Html(html) }
            }),
        );
        let parent_task =
            tokio::spawn(async move { axum::serve(parent_listener, parent_app).await.unwrap() });
        let script = r#"const {chromium}=require('playwright'); const fs=require('node:fs'); const path=require('node:path');
(async()=>{const browser=await chromium.launchPersistentContext(process.argv[2],{executablePath:'/usr/bin/chromium',args:['--no-sandbox','--no-proxy-server']});
try {const page=browser.pages()[0]||await browser.newPage(); let sockets=0; page.on('websocket',()=>sockets++);
page.on('pageerror',e=>console.log('PAGE ERROR',e.message));
page.on('console',m=>{if(m.type()==='error') console.log('CONSOLE ERROR',m.text())});
page.on('requestfailed',r=>console.log('REQUEST FAILED',r.url(),r.failure()?.errorText));
page.on('response',r=>{if(r.status()>=400) console.log('HTTP ERROR',r.status(),r.url())});
await page.goto(process.argv[1]); const frame=page.frameLocator('iframe');
try {await frame.locator('.monaco-workbench').waitFor({timeout:25000});}
catch(e){console.log('PAGE TEXT',(await frame.locator('body').innerText()).slice(0,1500));throw e;}
await frame.getByText('hello.py',{exact:true}).first().dblclick({timeout:30000});
await frame.getByText("print('container workbench')",{exact:false}).first().waitFor({timeout:30000});
await frame.locator('body').evaluate(()=>localStorage.setItem('autonomics-origin-test','persisted'));
await page.waitForTimeout(3000); if (!sockets) throw new Error('no remote websocket opened');
const dir=process.argv[3]; const wait=async name=>{for(let i=0;i<500;i++){if(fs.existsSync(path.join(dir,name)))return;await new Promise(r=>setTimeout(r,100));}throw Error('timeout '+name)};
fs.writeFileSync(path.join(dir,'browser-ready'),'1'); await wait('open-issued');
const line=frame.getByText("print('bridge opened file')",{exact:false}).first(); await line.waitFor({timeout:30000});
await line.click(); await page.keyboard.press('End'); await page.keyboard.type('#dirty');
await frame.getByText('#dirty',{exact:false}).first().waitFor({timeout:10000});
fs.writeFileSync(path.join(dir,'browser-dirty'),'1'); await wait('save-issued');
await page.keyboard.press('Control+s'); fs.writeFileSync(path.join(dir,'browser-saved'),'1'); await wait('browser-done');
console.log('workbench, file and remote websocket ready');
} finally {await browser.close();}})().catch(e=>{console.error(e);process.exit(1)});"#;
        let gui = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../apps/gui");
        let browser_profile = runtime_dir.path().join("browser-profile");
        let mut browser = tokio::process::Command::new("node")
            .current_dir(gui)
            .args(["-e", script, &parent_url])
            .arg(&browser_profile)
            .arg(runtime_dir.path())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .unwrap();
        let wait_file = |name: &'static str| {
            let path = runtime_dir.path().join(name);
            async move {
                timeout(Duration::from_secs(45), async {
                    while !path.exists() {
                        sleep(Duration::from_millis(100)).await;
                    }
                })
                .await
            }
        };
        if wait_file("browser-ready").await.is_err() {
            let _ = browser.kill().await;
            let output = browser.wait_with_output().await.unwrap();
            panic!(
                "browser failed: {}{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
        }
        let state_request = br#"{"action":"state","debug":true}"#;
        timeout(Duration::from_secs(20), async {
            loop {
                if manager
                    .vscode_control(&config, "vscode-test", state_request)
                    .await
                    .is_ok()
                {
                    break;
                }
                sleep(Duration::from_millis(200)).await;
            }
        })
        .await
        .expect("bridge activation");
        let opened = manager
            .vscode_control(
                &config,
                "vscode-test",
                br#"{"action":"open","path":"second.py","line":1,"column":1}"#,
            )
            .await
            .unwrap_or_else(|_| panic!("open file in VS Code"));
        assert_eq!(opened["opened"], true);
        std::fs::write(runtime_dir.path().join("open-issued"), "1").unwrap();
        if wait_file("browser-dirty").await.is_err() {
            let _ = browser.kill().await;
            let output = browser.wait_with_output().await.unwrap();
            panic!(
                "open/edit failed: {}{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
        }
        let dirty_state = timeout(Duration::from_secs(10), async {
            loop {
                if manager
                    .vscode_control(&config, "vscode-test", state_request)
                    .await
                    .is_ok_and(|value| value["dirty"] == true)
                {
                    break;
                }
                sleep(Duration::from_millis(200)).await;
            }
        })
        .await;
        if dirty_state.is_err() {
            let last = manager
                .vscode_control(&config, "vscode-test", state_request)
                .await
                .ok();
            let disk =
                std::fs::read_to_string(workspace.path().join("second.py")).unwrap_or_default();
            panic!("dirty state not reported: {last:?}; disk={disk:?}");
        }
        std::fs::write(runtime_dir.path().join("save-issued"), "1").unwrap();
        wait_file("browser-saved").await.expect("save in VS Code");
        timeout(Duration::from_secs(10), async {
            loop {
                if manager
                    .vscode_control(&config, "vscode-test", state_request)
                    .await
                    .is_ok_and(|value| value["dirty"] == false)
                {
                    break;
                }
                sleep(Duration::from_millis(200)).await;
            }
        })
        .await
        .expect("clean state");
        std::fs::write(runtime_dir.path().join("browser-done"), "1").unwrap();
        let output = timeout(Duration::from_secs(15), browser.wait_with_output())
            .await
            .unwrap()
            .unwrap();
        assert!(
            output.status.success(),
            "{}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        parent_task.abort();
    }
    assert!(manager.is_connected(&config, "vscode-test").await);
    let marker = manager.exec(&config, "vscode-test", "mkdir -p /var/autonomics-vscode-data/extensions/test-extension && printf persisted > /var/autonomics-vscode-data/extensions/test-extension/marker", &[], &[]).await.unwrap_or_else(|_| panic!("write VS Code extension data"));
    assert!(marker.status.success());
    manager.close("vscode-test").await;
    assert!(!socket_dir.exists());
    let previous = manager
        .connect(&config, "vscode-test", &image, workspace.path())
        .await
        .unwrap_or_else(|_| panic!("reopen VS Code container"));
    let reopened_url = {
        let slot = manager
            .active("vscode-test")
            .await
            .unwrap_or_else(|_| panic!("active reopened project"));
        slot.as_ref()
            .unwrap()
            .vscode
            .as_ref()
            .unwrap()
            .listener
            .url
            .clone()
    };
    assert_eq!(
        workbench_url.split('/').take(3).collect::<Vec<_>>(),
        reopened_url.split('/').take(3).collect::<Vec<_>>()
    );
    assert_ne!(workbench_url, reopened_url);
    if std::env::var_os("AUTONOMICS_DEVCONTAINER_TEST_BROWSER").is_some() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let parent_url = format!("http://{}/", listener.local_addr().unwrap());
        let html = format!(
            r#"<iframe src="{reopened_url}" sandbox="allow-scripts allow-same-origin allow-forms allow-modals allow-popups allow-downloads allow-pointer-lock" style="width:1400px;height:900px"></iframe>"#
        );
        let app = axum::Router::new().route(
            "/",
            axum::routing::get(move || {
                let html = html.clone();
                async move { axum::response::Html(html) }
            }),
        );
        let parent_task = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let script = r#"const {chromium}=require('playwright');
(async()=>{const ctx=await chromium.launchPersistentContext(process.argv[2],{executablePath:'/usr/bin/chromium',args:['--no-sandbox','--no-proxy-server']});
try {const page=ctx.pages()[0]||await ctx.newPage(); await page.goto(process.argv[1],{waitUntil:'domcontentloaded'});
const frame=page.frameLocator('iframe'); await frame.locator('.monaco-workbench').waitFor({timeout:25000});
if(await frame.locator('body').evaluate(()=>localStorage.getItem('autonomics-origin-test'))!=='persisted') throw new Error('browser storage not restored');
}finally{await ctx.close();}})().catch(e=>{console.error(e);process.exit(1)});"#;
        let output = timeout(
            Duration::from_secs(45),
            tokio::process::Command::new("node")
                .current_dir(Path::new(env!("CARGO_MANIFEST_DIR")).join("../../apps/gui"))
                .args(["-e", script, &parent_url])
                .arg(runtime_dir.path().join("browser-profile"))
                .output(),
        )
        .await
        .unwrap()
        .unwrap();
        assert!(
            output.status.success(),
            "{}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        parent_task.abort();
    }
    let marker = manager
        .exec(
            &config,
            "vscode-test",
            "cat /var/autonomics-vscode-data/extensions/test-extension/marker",
            &[],
            &[],
        )
        .await
        .unwrap_or_else(|_| panic!("read persisted data"));
    assert!(marker.status.success());
    assert_eq!(marker.stdout, b"persisted");
    let rebuilt = manager
        .connect_mode(&config, "vscode-test", &image, workspace.path(), true)
        .await
        .unwrap_or_else(|_| panic!("rebuild VS Code container"));
    assert_ne!(previous, rebuilt);
    let marker = manager
        .exec(
            &config,
            "vscode-test",
            "cat /var/autonomics-vscode-data/extensions/test-extension/marker",
            &[],
            &[],
        )
        .await
        .unwrap_or_else(|_| panic!("read data after rebuild"));
    assert!(marker.status.success());
    assert_eq!(marker.stdout, b"persisted");
    manager
        .connect(&config, "vscode-other", &image, workspace.path())
        .await
        .unwrap_or_else(|_| panic!("open second project"));
    let isolated = manager
        .exec(
            &config,
            "vscode-other",
            "test ! -e /var/autonomics-vscode-data/extensions/test-extension/marker",
            &[],
            &[],
        )
        .await
        .unwrap_or_else(|_| panic!("check project isolation"));
    assert!(isolated.status.success());
    manager.close("vscode-other").await;
    manager.close("vscode-test").await;
}

#[cfg(unix)]
#[tokio::test]
async fn temporary_health_probe_failure_does_not_destroy_session() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().unwrap();
    let marker = dir.path().join("fail");
    std::fs::write(&marker, "").unwrap();
    let program = dir.path().join("podman-mock");
    std::fs::write(
        &program,
        format!("#!/bin/sh\n[ ! -e '{}' ]\n", marker.display()),
    )
    .unwrap();
    std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o755)).unwrap();
    let mut config = PodmanConfig::default();
    config.program = program.to_string_lossy().into_owned();
    config.storage_root = None;
    config.run_root = None;
    config.storage_driver = None;
    let child = tokio::process::Command::new("/bin/sh")
        .args(["-c", "cat >/dev/null"])
        .stdin(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .unwrap();
    let manager = DevContainers::default();
    let slot = manager.slot("project", true).await.unwrap();
    *slot.lock().await = Some(Session {
        name: "container".into(),
        settings: super::super::devcontainer_config::DevConfig::load(dir.path(), "image")
            .unwrap_or_else(|_| panic!("default config")),
        workspace: dir.path().to_path_buf(),
        remote_env: BTreeMap::new(),
        child,
        terminal: None,
        language_servers: HashMap::new(),
        vscode: None,
    });
    assert!(!manager.is_connected(&config, "project").await);
    assert!(manager.active("project").await.is_ok());
    assert!(
        manager
            .connect(&config, "project", "image", dir.path())
            .await
            .is_err()
    );
    assert!(manager.active("project").await.is_ok());
    std::fs::remove_file(&marker).unwrap();
    assert!(manager.is_connected(&config, "project").await);
    assert!(
        matches!(manager.connect(&config, "project", "image", dir.path()).await, Ok(name) if name == "container")
    );
    manager.close("project").await;
}

#[cfg(unix)]
#[tokio::test]
async fn slow_language_start_does_not_block_session_or_survive_close() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().unwrap();
    let marker = dir.path().join("probe-started");
    let program = dir.path().join("podman-mock");
    std::fs::write(&program, format!("#!/bin/sh\ncase \"$*\" in *--interactive*) exec cat;; *) touch '{}'; sleep 2; exit 0;; esac\n", marker.display())).unwrap();
    std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o755)).unwrap();
    let mut config = PodmanConfig::default();
    config.program = program.to_string_lossy().into_owned();
    config.storage_root = None;
    config.run_root = None;
    config.storage_driver = None;
    let child = tokio::process::Command::new("/bin/sh")
        .args(["-c", "cat >/dev/null"])
        .stdin(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .unwrap();
    let manager = Arc::new(DevContainers::default());
    let slot = manager.slot("project", true).await.unwrap();
    *slot.lock().await = Some(Session {
        name: "container".into(),
        settings: super::super::devcontainer_config::DevConfig::load(dir.path(), "image")
            .unwrap_or_else(|_| panic!("default config")),
        workspace: dir.path().to_path_buf(),
        remote_env: BTreeMap::new(),
        child,
        terminal: None,
        language_servers: HashMap::new(),
        vscode: None,
    });
    let task = {
        let manager = manager.clone();
        tokio::spawn(async move { manager.open_language(&config, "project", "python").await })
    };
    timeout(Duration::from_secs(5), async {
        while !marker.exists() {
            sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("probe started");
    let guard = timeout(Duration::from_millis(500), manager.active("project"))
        .await
        .expect("session remained responsive")
        .unwrap_or_else(|_| panic!("session missing"));
    drop(guard);
    timeout(Duration::from_secs(1), manager.close("project"))
        .await
        .expect("close remained responsive");
    assert!(matches!(task.await, Ok(Err(_))));
    assert!(manager.active("project").await.is_err());
}

#[cfg(unix)]
#[tokio::test]
async fn slow_snapshot_does_not_block_session() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().unwrap();
    let marker = dir.path().join("scan-started");
    let program = dir.path().join("podman-mock");
    std::fs::write(
        &program,
        format!("#!/bin/sh\ntouch '{}'; sleep 2\n", marker.display()),
    )
    .unwrap();
    std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o755)).unwrap();
    let mut config = PodmanConfig::default();
    config.program = program.to_string_lossy().into_owned();
    config.storage_root = None;
    config.run_root = None;
    config.storage_driver = None;
    let child = tokio::process::Command::new("/bin/sh")
        .args(["-c", "cat >/dev/null"])
        .stdin(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .unwrap();
    let manager = Arc::new(DevContainers::default());
    let slot = manager.slot("project", true).await.unwrap();
    *slot.lock().await = Some(Session {
        name: "container".into(),
        settings: super::super::devcontainer_config::DevConfig::load(dir.path(), "image")
            .unwrap_or_else(|_| panic!("default config")),
        workspace: dir.path().to_path_buf(),
        remote_env: BTreeMap::new(),
        child,
        terminal: None,
        language_servers: HashMap::new(),
        vscode: None,
    });
    let task = {
        let manager = manager.clone();
        tokio::spawn(async move { manager.snapshot(&config, "project").await })
    };
    timeout(Duration::from_secs(5), async {
        while !marker.exists() {
            sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("snapshot started");
    let guard = timeout(Duration::from_millis(500), manager.active("project"))
        .await
        .expect("snapshot must not block session")
        .unwrap_or_else(|_| panic!("session missing"));
    drop(guard);
    timeout(Duration::from_secs(1), manager.close("project"))
        .await
        .expect("close remained responsive");
    assert!(matches!(task.await, Ok(Ok(_))));
}

#[cfg(unix)]
#[tokio::test]
async fn oversized_snapshot_is_rejected() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().unwrap();
    let program = dir.path().join("podman-mock");
    std::fs::write(&program, "#!/bin/sh\nhead -c 8388609 /dev/zero\n").unwrap();
    std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o755)).unwrap();
    let mut config = PodmanConfig::default();
    config.program = program.to_string_lossy().into_owned();
    config.storage_root = None;
    config.run_root = None;
    config.storage_driver = None;
    let child = tokio::process::Command::new("/bin/sh")
        .args(["-c", "cat >/dev/null"])
        .stdin(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .unwrap();
    let manager = DevContainers::default();
    let slot = manager.slot("project", true).await.unwrap();
    *slot.lock().await = Some(Session {
        name: "container".into(),
        settings: super::super::devcontainer_config::DevConfig::load(dir.path(), "image")
            .unwrap_or_else(|_| panic!("default config")),
        workspace: dir.path().to_path_buf(),
        remote_env: BTreeMap::new(),
        child,
        terminal: None,
        language_servers: HashMap::new(),
        vscode: None,
    });
    assert!(manager.snapshot(&config, "project").await.is_err());
    manager.close("project").await;
}

#[tokio::test]
#[ignore = "requires AUTONOMICS_DEVCONTAINER_TEST_STATE and AUTONOMICS_DEVCONTAINER_TEST_ALPINE_IMAGE"]
async fn real_container_snapshot_busybox() {
    let state = std::env::var("AUTONOMICS_DEVCONTAINER_TEST_STATE").expect("test state directory");
    let image =
        std::env::var("AUTONOMICS_DEVCONTAINER_TEST_ALPINE_IMAGE").expect("local Alpine image");
    let config =
        container_runtime::ContainerExecutionInfra::for_operator_state_dir(Path::new(&state))
            .expect("container configuration")
            .config;
    let workspace = tempfile::tempdir().unwrap();
    std::fs::write(workspace.path().join("a\nfile"), "data").unwrap();
    let manager = DevContainers::default();
    manager
        .connect(&config, "alpine-snapshot", &image, workspace.path())
        .await
        .unwrap_or_else(|_| panic!("Alpine container"));
    let output = manager
        .snapshot(&config, "alpine-snapshot")
        .await
        .unwrap_or_else(|_| panic!("Alpine snapshot"));
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let entries = super::super::plugins::parse_project_snapshot(&output.stdout)
        .unwrap_or_else(|_| panic!("parse Alpine snapshot"));
    assert!(
        entries
            .iter()
            .any(|entry| entry["path"] == "a\nfile" && entry["kind"] == "file")
    );
    manager.close("alpine-snapshot").await;
}

#[tokio::test]
#[ignore = "requires AUTONOMICS_DEVCONTAINER_TEST_STATE and AUTONOMICS_DEVCONTAINER_TEST_IMAGE"]
async fn real_container_uses_configured_remote_user() {
    let state = std::env::var("AUTONOMICS_DEVCONTAINER_TEST_STATE").expect("test state directory");
    let image = std::env::var("AUTONOMICS_DEVCONTAINER_TEST_IMAGE").expect("local test image");
    let mut config =
        container_runtime::ContainerExecutionInfra::for_operator_state_dir(Path::new(&state))
            .expect("container configuration")
            .config;
    let runtime_dir = tempfile::tempdir().unwrap();
    config.workspace_root = runtime_dir.path().join("runtime");
    let workspace = tempfile::tempdir().unwrap();
    std::fs::write(workspace.path().join(".devcontainer.json"), r#"{"containerUser":"root","remoteUser":"nobody","onCreateCommand":"id -u > /work/lifecycle-uid"}"#).unwrap();
    let manager = DevContainers::default();
    let name = manager
        .connect(&config, "user-project", &image, workspace.path())
        .await
        .unwrap_or_else(|_| panic!("container with remote user failed to start"));
    assert_eq!(
        std::fs::read_to_string(workspace.path().join("lifecycle-uid"))
            .unwrap()
            .trim(),
        "65534"
    );
    let output = manager
        .exec(
            &config,
            "user-project",
            "id -u; printf writable > /work/user-file",
            &[],
            &[],
        )
        .await
        .unwrap_or_else(|_| panic!("remote user exec"));
    assert!(output.status.success());
    assert_eq!(String::from_utf8_lossy(&output.stdout).trim(), "65534");
    assert_eq!(
        std::fs::read_to_string(workspace.path().join("user-file")).unwrap(),
        "writable"
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        assert_eq!(
            std::fs::metadata(workspace.path().join("user-file"))
                .unwrap()
                .uid(),
            std::fs::metadata(workspace.path()).unwrap().uid()
        );
    }
    let container_user = config
        .async_command()
        .args(["exec", &name, "/bin/sh", "-c", "id -u"])
        .output()
        .await
        .unwrap();
    assert_eq!(String::from_utf8_lossy(&container_user.stdout).trim(), "0");
    let terminal = super::super::devterminal::Terminal::start(
        &config,
        &name,
        24,
        80,
        &BTreeMap::new(),
        Some("nobody"),
    )
    .expect("terminal");
    terminal
        .write("id -u > /work/terminal-uid\n".into())
        .unwrap();
    timeout(Duration::from_secs(10), async {
        while std::fs::read_to_string(workspace.path().join("terminal-uid"))
            .map_or(true, |value| value.trim() != "65534")
        {
            sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .expect("terminal ran");
    assert_eq!(
        std::fs::read_to_string(workspace.path().join("terminal-uid"))
            .unwrap()
            .trim(),
        "65534"
    );
    drop(terminal);
    let mut language = super::super::devlsp::LanguageServer::launch(
        &config,
        &name,
        "id -u > /work/language-uid; exec cat",
        &BTreeMap::new(),
        Some("nobody"),
    )
    .expect("language transport");
    language.send(json!({"id":1})).await.unwrap();
    timeout(Duration::from_secs(10), async {
        while std::fs::read_to_string(workspace.path().join("language-uid"))
            .map_or(true, |value| value.trim() != "65534")
        {
            sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .expect("language ran");
    assert_eq!(
        std::fs::read_to_string(workspace.path().join("language-uid"))
            .unwrap()
            .trim(),
        "65534"
    );
    drop(language);
    manager.close("user-project").await;
    std::fs::write(
        workspace.path().join(".devcontainer.json"),
        r#"{"containerUser":"nobody"}"#,
    )
    .unwrap();
    let name = manager
        .connect(&config, "user-project", &image, workspace.path())
        .await
        .unwrap_or_else(|_| panic!("container user failed to start"));
    let output = config
        .async_command()
        .args(["exec", &name, "/bin/sh", "-c", "id -u"])
        .output()
        .await
        .unwrap();
    assert_eq!(String::from_utf8_lossy(&output.stdout).trim(), "65534");
    manager.close("user-project").await;
    std::fs::write(
        workspace.path().join(".devcontainer.json"),
        r#"{"remoteUser":"nobody"}"#,
    )
    .unwrap();
    let name = manager
        .connect(&config, "user-project", &image, workspace.path())
        .await
        .unwrap_or_else(|_| panic!("image default container user"));
    let output = config
        .async_command()
        .args(["exec", &name, "/bin/sh", "-c", "id -u"])
        .output()
        .await
        .unwrap();
    assert_eq!(String::from_utf8_lossy(&output.stdout).trim(), "0");
    let output = manager
        .exec(&config, "user-project", "id -u", &[], &[])
        .await
        .unwrap_or_else(|_| panic!("remote user"));
    assert_eq!(String::from_utf8_lossy(&output.stdout).trim(), "65534");
    manager.close("user-project").await;
}

#[tokio::test]
#[ignore = "requires AUTONOMICS_DEVCONTAINER_TEST_STATE and AUTONOMICS_DEVCONTAINER_TEST_IMAGE"]
async fn real_container_builds_project_dockerfile() {
    let state = std::env::var("AUTONOMICS_DEVCONTAINER_TEST_STATE").expect("test state directory");
    let image = std::env::var("AUTONOMICS_DEVCONTAINER_TEST_IMAGE").expect("local test image");
    let config =
        container_runtime::ContainerExecutionInfra::for_operator_state_dir(Path::new(&state))
            .expect("container configuration")
            .config;
    let workspace = tempfile::tempdir().unwrap();
    let folder = workspace.path().join(".devcontainer");
    std::fs::create_dir(&folder).unwrap();
    std::fs::write(workspace.path().join("marker"), "from-context").unwrap();
    let dockerfile = folder.join("Dockerfile");
    std::fs::write(&dockerfile, format!("FROM {image} AS development\nARG MESSAGE\nENV BUILD_MESSAGE=$MESSAGE\nCOPY marker /build-marker\n")).unwrap();
    let config_path = folder.join("devcontainer.json");
    std::fs::write(&config_path, r#"{"build":{"dockerfile":"Dockerfile","context":"..","args":{"MESSAGE":"hello"},"target":"development"}}"#).unwrap();
    let manager = DevContainers::default();
    let name = manager
        .connect(&config, "build-project", &image, workspace.path())
        .await
        .unwrap_or_else(|_| panic!("built container failed to start"));
    let output = manager
        .exec(
            &config,
            "build-project",
            "printf '%s:' \"$BUILD_MESSAGE\"; cat /build-marker",
            &[],
            &[],
        )
        .await
        .unwrap_or_else(|_| panic!("built image contents"));
    assert_eq!(output.stdout, b"hello:from-context");
    std::fs::write(workspace.path().join("marker"), "updated-context").unwrap();
    let rebuilt = manager
        .connect_mode(&config, "build-project", &image, workspace.path(), true)
        .await
        .unwrap_or_else(|_| panic!("force rebuild"));
    assert_ne!(name, rebuilt);
    let output = manager
        .exec(&config, "build-project", "cat /build-marker", &[], &[])
        .await
        .unwrap_or_else(|_| panic!("rebuilt image contents"));
    assert_eq!(output.stdout, b"updated-context");
    std::fs::write(&dockerfile, "NOT_A_DOCKERFILE invalid\n").unwrap();
    std::fs::write(
        &config_path,
        r#"{"build":{"dockerfile":"Dockerfile","context":"..","args":{"MESSAGE":"changed"}}}"#,
    )
    .unwrap();
    assert!(
        manager
            .connect(&config, "build-project", &image, workspace.path())
            .await
            .is_err()
    );
    assert!(running(&config, &rebuilt).await);
    assert!(manager.active("build-project").await.is_ok());
    manager.close("build-project").await;
}

/// Opt-in integration test: run against an operator-selected local store
/// and image, using an isolated temporary workspace and --rm container.
#[tokio::test]
#[ignore = "requires AUTONOMICS_DEVCONTAINER_TEST_STATE and AUTONOMICS_DEVCONTAINER_TEST_IMAGE"]
async fn real_container_reads_and_writes_mounted_project() {
    let state = std::env::var("AUTONOMICS_DEVCONTAINER_TEST_STATE").expect("test state directory");
    let image = std::env::var("AUTONOMICS_DEVCONTAINER_TEST_IMAGE").expect("local test image");
    let config =
        container_runtime::ContainerExecutionInfra::for_operator_state_dir(Path::new(&state))
            .expect("container configuration")
            .config;
    let workspace = tempfile::tempdir().expect("temporary workspace");
    std::fs::write(workspace.path().join("input.txt"), "before").unwrap();
    std::fs::create_dir(workspace.path().join(".devcontainer")).unwrap();
    std::fs::write(
            workspace.path().join(".devcontainer/devcontainer.json"),
            r#"{
          // Commands and variables are evaluated inside the project container.
          "containerEnv": {"DEV_TEST": "container-${containerWorkspaceFolderBasename}"},
          "remoteEnv": {"REMOTE_TEST": "${containerEnv:DEV_TEST}-remote", "PATH": "${containerEnv:PATH}"},
          "onCreateCommand": "printf %s \"$REMOTE_TEST\" > /work/.remote-lifecycle; printf create\\\\n >> /work/.lifecycle",
          "updateContentCommand": ["/bin/sh", "-c", "printf update\\\\n >> /work/.lifecycle"],
          "postCreateCommand": "printf postcreate\\\\n >> /work/.lifecycle",
          "postStartCommand": "printf start\\\\n >> /work/.lifecycle",
          "postAttachCommand": "printf attach\\\\n >> /work/.lifecycle",
        }"#,
        )
        .unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(
            workspace.path().join("input.txt"),
            std::fs::Permissions::from_mode(0o755),
        )
        .unwrap();
    }
    let manager = Arc::new(DevContainers::default());
    let first = match manager
        .connect(&config, "test-project", &image, workspace.path())
        .await
    {
        Ok(name) => name,
        Err(_) => panic!("container did not start"),
    };
    std::fs::create_dir_all(workspace.path().join("node_modules/test-package")).unwrap();
    std::fs::write(
        workspace
            .path()
            .join("node_modules/test-package/ignored.js"),
        "ignored",
    )
    .unwrap();
    std::fs::write(workspace.path().join("name\nwith-newline.txt"), "value").unwrap();
    let snapshot = manager
        .snapshot(&config, "test-project")
        .await
        .unwrap_or_else(|_| panic!("container snapshot"));
    assert!(snapshot.status.success());
    let entries = super::super::plugins::parse_project_snapshot(&snapshot.stdout)
        .unwrap_or_else(|_| panic!("parse snapshot"));
    assert!(
        entries
            .iter()
            .any(|entry| entry["path"] == "input.txt" && entry["kind"] == "file")
    );
    assert!(
        entries
            .iter()
            .any(|entry| entry["path"] == "name\nwith-newline.txt")
    );
    assert!(!entries.iter().any(|entry| {
        entry["path"]
            .as_str()
            .is_some_and(|path| path.starts_with("node_modules"))
    }));
    assert!(super::super::plugins::parse_project_snapshot(b"f\0./bad\0partial").is_err());
    let second = match manager
        .connect(&config, "test-project", &image, workspace.path())
        .await
    {
        Ok(name) => name,
        Err(_) => panic!("container did not reconnect"),
    };
    assert_eq!(first, second);
    let env = manager
        .exec(&config, "test-project", "printf %s \"$DEV_TEST\"", &[], &[])
        .await
        .unwrap_or_else(|_| panic!("read container env"));
    assert_eq!(env.stdout, b"container-work");
    let remote = manager
        .exec(
            &config,
            "test-project",
            "printf %s \"$REMOTE_TEST\"",
            &[],
            &[],
        )
        .await
        .unwrap_or_else(|_| panic!("read remote env"));
    assert_eq!(remote.stdout, b"container-work-remote");
    assert_eq!(
        std::fs::read_to_string(workspace.path().join(".remote-lifecycle")).unwrap(),
        "container-work-remote"
    );
    let container_only = config
        .async_command()
        .args([
            "exec",
            &first,
            "/bin/sh",
            "-c",
            "printf %s \"${REMOTE_TEST-unset}\"",
        ])
        .output()
        .await
        .unwrap();
    assert_eq!(container_only.stdout, b"unset");
    let remote_env = manager
        .active("test-project")
        .await
        .unwrap_or_else(|_| panic!("active session"))
        .as_ref()
        .unwrap()
        .remote_env
        .clone();
    let lifecycle = std::fs::read_to_string(workspace.path().join(".lifecycle")).unwrap();
    assert_eq!(
        lifecycle,
        "create\nupdate\npostcreate\nstart\nattach\nattach\n"
    );
    let config_path = workspace.path().join(".devcontainer/devcontainer.json");
    let original_config = std::fs::read_to_string(&config_path).unwrap();
    std::fs::write(
        &config_path,
        original_config.replace(
            "${containerEnv:DEV_TEST}-remote",
            "${containerEnv:DEV_TEST}-updated",
        ),
    )
    .unwrap();
    let reconnected = manager
        .connect(&config, "test-project", &image, workspace.path())
        .await
        .unwrap_or_else(|_| panic!("reconnect after remoteEnv change"));
    assert_eq!(reconnected, first);
    let changed = manager
        .exec(
            &config,
            "test-project",
            "printf %s \"$REMOTE_TEST\"",
            &[],
            &[],
        )
        .await
        .unwrap_or_else(|_| panic!("read changed remote env"));
    assert_eq!(changed.stdout, b"container-work-updated");
    std::fs::write(&config_path, original_config).unwrap();
    let restored = manager
        .connect(&config, "test-project", &image, workspace.path())
        .await
        .unwrap_or_else(|_| panic!("restore remote env"));
    assert_eq!(restored, first);
    let other_workspace = tempfile::tempdir().expect("second workspace");
    if manager
        .connect(&config, "other-project", &image, other_workspace.path())
        .await
        .is_err()
    {
        panic!("second container did not start");
    }
    let slow_manager = manager.clone();
    let slow_config = config.clone();
    let slow = tokio::spawn(async move {
        slow_manager
            .exec(
                &slow_config,
                "test-project",
                "touch /work/slow-started; sleep 5",
                &[],
                &[],
            )
            .await
    });
    timeout(Duration::from_secs(10), async {
        while !workspace.path().join("slow-started").exists() {
            sleep(Duration::from_millis(25)).await;
        }
    })
    .await
    .expect("slow operation started");
    let other = timeout(
        Duration::from_secs(3),
        manager.exec(&config, "other-project", "printf responsive", &[], &[]),
    )
    .await
    .expect("another project must remain responsive");
    let other = match other {
        Ok(output) => output,
        Err(_) => panic!("second project operation failed"),
    };
    assert_eq!(other.stdout, b"responsive");
    assert!(matches!(slow.await, Ok(Ok(output)) if output.status.success()));
    manager.close("other-project").await;
    let output = match manager
        .exec(
            &config,
            "test-project",
            "cat /work/input.txt; printf after > /work/output.txt",
            &[],
            &[],
        )
        .await
    {
        Ok(output) => output,
        Err(_) => panic!("container exec failed"),
    };
    assert!(output.status.success());
    assert_eq!(output.stdout, b"before");
    assert_eq!(
        std::fs::read_to_string(workspace.path().join("output.txt")).unwrap(),
        "after"
    );
    let output = match manager
        .exec(&config, "test-project", &read_script(), &["input.txt"], &[])
        .await
    {
        Ok(output) => output,
        Err(_) => panic!("read failed"),
    };
    assert_eq!(output.stdout, b"before");
    let output = match manager
        .exec(
            &config,
            "test-project",
            &save_script(),
            &["input.txt", "6"],
            b"beforechanged",
        )
        .await
    {
        Ok(output) => output,
        Err(_) => panic!("save failed"),
    };
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        std::fs::read_to_string(workspace.path().join("input.txt")).unwrap(),
        "changed"
    );
    let output = match manager
        .exec(
            &config,
            "test-project",
            &create_directory_script(),
            &["empty-folder"],
            &[],
        )
        .await
    {
        Ok(output) => output,
        Err(_) => panic!("create directory failed"),
    };
    assert!(output.status.success());
    let output = match manager
        .exec(
            &config,
            "test-project",
            &create_directory_script(),
            &["empty-folder"],
            &[],
        )
        .await
    {
        Ok(output) => output,
        Err(_) => panic!("duplicate directory failed"),
    };
    assert_eq!(output.status.code(), Some(42));
    let output = match manager
        .exec(&config, "test-project", LIST_DIRECTORIES_SCRIPT, &[], &[])
        .await
    {
        Ok(output) => output,
        Err(_) => panic!("list directories failed"),
    };
    assert!(
        output
            .stdout
            .split(|byte| *byte == 0)
            .any(|path| path == b"./empty-folder")
    );
    let output = manager
        .exec(
            &config,
            "test-project",
            &create_directory_script(),
            &["folder"],
            &[],
        )
        .await
        .unwrap_or_else(|_| panic!("create folder failed"));
    assert!(output.status.success());
    let output = manager
        .exec(
            &config,
            "test-project",
            &create_script(),
            &["folder/child.txt"],
            &[],
        )
        .await
        .unwrap_or_else(|_| panic!("create nested file failed"));
    assert!(output.status.success());
    let output = manager
        .exec(
            &config,
            "test-project",
            &rename_directory_script(),
            &["folder", "empty-folder"],
            &[],
        )
        .await
        .unwrap_or_else(|_| panic!("folder conflict failed"));
    assert_eq!(output.status.code(), Some(42));
    assert!(workspace.path().join("folder/child.txt").exists());
    let output = manager
        .exec(
            &config,
            "test-project",
            &rename_directory_script(),
            &["folder", "moved-folder"],
            &[],
        )
        .await
        .unwrap_or_else(|_| panic!("folder rename failed"));
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(workspace.path().join("moved-folder/child.txt").exists());
    let output = manager
        .exec(
            &config,
            "test-project",
            &delete_directory_script(),
            &["moved-folder"],
            &[],
        )
        .await
        .unwrap_or_else(|_| panic!("folder delete failed"));
    assert!(output.status.success());
    assert!(!workspace.path().join("moved-folder").exists());
    let output = match manager
        .exec(&config, "test-project", &create_script(), &["new.txt"], &[])
        .await
    {
        Ok(output) => output,
        Err(_) => panic!("create failed"),
    };
    assert!(output.status.success());
    let output = match manager
        .exec(&config, "test-project", &create_script(), &["new.txt"], &[])
        .await
    {
        Ok(output) => output,
        Err(_) => panic!("duplicate create failed"),
    };
    assert_eq!(output.status.code(), Some(42));
    let output = match manager
        .exec(
            &config,
            "test-project",
            &rename_script(),
            &["new.txt", "input.txt"],
            &[],
        )
        .await
    {
        Ok(output) => output,
        Err(_) => panic!("rename conflict failed"),
    };
    assert_eq!(output.status.code(), Some(42));
    assert_eq!(
        std::fs::read_to_string(workspace.path().join("input.txt")).unwrap(),
        "changed"
    );
    let output = match manager
        .exec(
            &config,
            "test-project",
            &rename_script(),
            &["new.txt", "renamed.txt"],
            &[],
        )
        .await
    {
        Ok(output) => output,
        Err(_) => panic!("rename failed"),
    };
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(!workspace.path().join("new.txt").exists());
    assert!(workspace.path().join("renamed.txt").exists());
    let output = match manager
        .exec(
            &config,
            "test-project",
            &delete_script(),
            &["renamed.txt"],
            &[],
        )
        .await
    {
        Ok(output) => output,
        Err(_) => panic!("delete failed"),
    };
    assert!(output.status.success());
    assert!(!workspace.path().join("renamed.txt").exists());
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(workspace.path().join("input.txt"))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o755
        );
    }
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink("input.txt", workspace.path().join("link.txt")).unwrap();
        let output = match manager
            .exec(&config, "test-project", &read_script(), &["link.txt"], &[])
            .await
        {
            Ok(output) => output,
            Err(_) => panic!("symlink check failed"),
        };
        assert!(!output.status.success());
        let listing = match manager
            .exec(&config, "test-project", LIST_SCRIPT, &[], &[])
            .await
        {
            Ok(output) => output,
            Err(_) => panic!("list files"),
        };
        assert!(listing.status.success());
        assert!(
            !listing
                .stdout
                .split(|byte| *byte == 0)
                .any(|file| file == b"./link.txt")
        );
        std::os::unix::fs::symlink("/tmp", workspace.path().join("linkdir")).unwrap();
        let output = match manager
            .exec(
                &config,
                "test-project",
                &create_script(),
                &["linkdir/escaped"],
                &[],
            )
            .await
        {
            Ok(output) => output,
            Err(_) => panic!("symlink create check failed"),
        };
        assert!(!output.status.success());
        let output = match manager
            .exec(
                &config,
                "test-project",
                &rename_script(),
                &["input.txt", "linkdir/escaped"],
                &[],
            )
            .await
        {
            Ok(output) => output,
            Err(_) => panic!("symlink rename check failed"),
        };
        assert!(!output.status.success());
        assert_eq!(
            std::fs::read_to_string(workspace.path().join("input.txt")).unwrap(),
            "changed"
        );
    }
    let output = match manager
        .exec(
            &config,
            "test-project",
            &save_script(),
            &["input.txt", "6"],
            b"beforeoverwritten",
        )
        .await
    {
        Ok(output) => output,
        Err(_) => panic!("conflict check failed"),
    };
    assert_eq!(output.status.code(), Some(42));
    assert_eq!(
        std::fs::read_to_string(workspace.path().join("input.txt")).unwrap(),
        "changed"
    );
    let mut terminal =
        super::super::devterminal::Terminal::start(&config, &first, 24, 80, &remote_env, None)
            .expect("start terminal");
    terminal.resize(32, 100).expect("resize terminal");
    terminal
        .write("printf %s \"$REMOTE_TEST\" > /work/terminal-proof\n".into())
        .expect("terminal input");
    timeout(Duration::from_secs(10), async {
        loop {
            if workspace.path().join("terminal-proof").exists() {
                break;
            }
            sleep(Duration::from_millis(100)).await;
        }
    })
    .await
    .expect("terminal command executed");
    assert_eq!(
        std::fs::read_to_string(workspace.path().join("terminal-proof")).unwrap(),
        "container-work-remote"
    );
    assert!(!terminal.exited().expect("terminal status"));
    drop(terminal);
    let mut language = super::super::devlsp::LanguageServer::launch(
        &config,
        &first,
        "printf %s \"$REMOTE_TEST\" > /work/lsp-env; exec cat",
        &remote_env,
        None,
    )
    .expect("language transport");
    let message = json!({"jsonrpc":"2.0", "id":7, "method":"initialize", "params":{"rootUri":"file:///work"}});
    language
        .send(message.clone())
        .await
        .expect("language input");
    let response = timeout(Duration::from_secs(10), async {
        loop {
            let messages = language.poll(0).unwrap().0;
            if !messages.is_empty() {
                break messages;
            }
            sleep(Duration::from_millis(30)).await;
        }
    })
    .await
    .expect("language output");
    assert_eq!(response, vec![message]);
    assert_eq!(
        std::fs::read_to_string(workspace.path().join("lsp-env")).unwrap(),
        "container-work-remote"
    );
    drop(language);
    if std::env::var_os("AUTONOMICS_DEVCONTAINER_TEST_INSTALL_TS").is_some() {
        let node = std::env::var("AUTONOMICS_DEVCONTAINER_TEST_NODE_BIN")
            .expect("host Node binary for isolated test");
        let bin = workspace.path().join(".test-bin");
        std::fs::create_dir(&bin).unwrap();
        std::fs::copy(node, bin.join("node")).unwrap();
        let install = timeout(
            Duration::from_secs(180),
            tokio::process::Command::new("npm")
                .args([
                    "install",
                    "--no-save",
                    "--ignore-scripts",
                    "--no-audit",
                    "--no-fund",
                    "--prefix",
                ])
                .arg(workspace.path())
                .arg("--cache")
                .arg(workspace.path().join(".npm-cache"))
                .args(["typescript@5.9.3", "typescript-language-server@4.4.1"])
                .stdin(Stdio::null())
                .kill_on_drop(true)
                .output(),
        )
        .await
        .expect("npm install timeout")
        .expect("install TypeScript test dependencies");
        assert!(
            install.status.success(),
            "{}",
            String::from_utf8_lossy(&install.stderr)
        );
        let mut ts_env = remote_env.clone();
        let path = ts_env
            .get("PATH")
            .cloned()
            .unwrap_or_else(|| "/usr/local/bin:/usr/bin:/bin".into());
        ts_env.insert("PATH".into(), format!("/work/.test-bin:{path}"));
        let mut server = super::super::devlsp::LanguageServer::start(
            &config,
            &first,
            "typescript",
            &ts_env,
            None,
        )
        .await
        .expect("start TypeScript language server in container");
        server.send(json!({"jsonrpc":"2.0","id":10,"method":"initialize","params":{"processId":null,"rootUri":"file:///work","capabilities":{}}})).await.unwrap();
        let mut cursor = 0;
        let initialized = timeout(Duration::from_secs(30), async {
            loop {
                let (messages, next, _, _) = server.poll(cursor).unwrap();
                cursor = next;
                if let Some(message) = messages.into_iter().find(|message| message["id"] == 10) {
                    break message;
                }
                sleep(Duration::from_millis(50)).await;
            }
        })
        .await
        .expect("TypeScript initialization response");
        assert!(
            initialized["result"]["capabilities"]["completionProvider"].is_object(),
            "{initialized}"
        );
        server
            .send(json!({"jsonrpc":"2.0","method":"initialized","params":{}}))
            .await
            .unwrap();
        server.send(json!({"jsonrpc":"2.0","method":"textDocument/didOpen","params":{"textDocument":{"uri":"file:///work/example.ts","languageId":"typescript","version":1,"text":"const greeting: string = 'hello';\ngree"}}})).await.unwrap();
        server.send(json!({"jsonrpc":"2.0","id":11,"method":"textDocument/completion","params":{"textDocument":{"uri":"file:///work/example.ts"},"position":{"line":1,"character":4}}})).await.unwrap();
        let completion = timeout(Duration::from_secs(30), async {
            loop {
                let (messages, next, _, _) = server.poll(cursor).unwrap();
                cursor = next;
                if let Some(message) = messages.into_iter().find(|message| message["id"] == 11) {
                    break message;
                }
                sleep(Duration::from_millis(50)).await;
            }
        })
        .await
        .expect("TypeScript completion response");
        let items = completion["result"]["items"]
            .as_array()
            .or_else(|| completion["result"].as_array())
            .expect("TypeScript completion items");
        assert!(
            items.iter().any(|item| item["label"] == "greeting"),
            "{completion}"
        );
        drop(server);
        let mut js_server = super::super::devlsp::LanguageServer::start(
            &config,
            &first,
            "javascript",
            &ts_env,
            None,
        )
        .await
        .expect("start JavaScript language server in container");
        js_server.send(json!({"jsonrpc":"2.0","id":12,"method":"initialize","params":{"processId":null,"rootUri":"file:///work","capabilities":{}}})).await.unwrap();
        let initialized = timeout(Duration::from_secs(30), async {
            loop {
                let (messages, _, _, _) = js_server.poll(0).unwrap();
                if let Some(message) = messages.into_iter().find(|message| message["id"] == 12) {
                    break message;
                }
                sleep(Duration::from_millis(50)).await;
            }
        })
        .await
        .expect("JavaScript initialization response");
        assert!(
            initialized["result"]["capabilities"]["completionProvider"].is_object(),
            "{initialized}"
        );
        drop(js_server);
    }
    if std::env::var_os("AUTONOMICS_DEVCONTAINER_TEST_INSTALL_CLANGD").is_some() {
        let install = timeout(Duration::from_secs(300), config.async_command().args(["exec", "--user=0", "--workdir=/", &first, "/bin/sh", "-c", "mkdir -p /tmp/apt-archives/partial && apt-get -o APT::Sandbox::User=root update -qq && DEBIAN_FRONTEND=noninteractive apt-get -o APT::Sandbox::User=root -o Dir::Cache::archives=/tmp/apt-archives install -y -qq --no-install-recommends clangd"]).stdin(Stdio::null()).kill_on_drop(true).output()).await.expect("clangd install timeout").expect("install clangd in test container");
        assert!(
            install.status.success(),
            "{}",
            String::from_utf8_lossy(&install.stderr)
        );
        for (index, language) in ["c", "cpp"].iter().enumerate() {
            let mut server = super::super::devlsp::LanguageServer::start(
                &config,
                &first,
                language,
                &remote_env,
                None,
            )
            .await
            .expect("start clangd in container");
            let init_id = 30 + index * 2;
            server.send(json!({"jsonrpc":"2.0","id":init_id,"method":"initialize","params":{"processId":null,"rootUri":"file:///work","capabilities":{}}})).await.unwrap();
            let mut cursor = 0;
            let initialized = timeout(Duration::from_secs(30), async {
                loop {
                    let (messages, next, _, _) = server.poll(cursor).unwrap();
                    cursor = next;
                    if let Some(message) = messages
                        .into_iter()
                        .find(|message| message["id"] == init_id)
                    {
                        break message;
                    }
                    sleep(Duration::from_millis(50)).await;
                }
            })
            .await
            .expect("clangd initialize");
            assert!(
                initialized["result"]["capabilities"]["completionProvider"].is_object(),
                "{initialized}"
            );
            server
                .send(json!({"jsonrpc":"2.0","method":"initialized","params":{}}))
                .await
                .unwrap();
            let uri = format!(
                "file:///work/example.{}",
                if *language == "c" { "c" } else { "cpp" }
            );
            server.send(json!({"jsonrpc":"2.0","method":"textDocument/didOpen","params":{"textDocument":{"uri":uri,"languageId":language,"version":1,"text":"int greeting = 1;\nint x = greet"}}})).await.unwrap();
            let completion_id = init_id + 1;
            server.send(json!({"jsonrpc":"2.0","id":completion_id,"method":"textDocument/completion","params":{"textDocument":{"uri":uri},"position":{"line":1,"character":13}}})).await.unwrap();
            let completion = timeout(Duration::from_secs(30), async {
                loop {
                    let (messages, next, _, _) = server.poll(cursor).unwrap();
                    cursor = next;
                    if let Some(message) = messages
                        .into_iter()
                        .find(|message| message["id"] == completion_id)
                    {
                        break message;
                    }
                    sleep(Duration::from_millis(50)).await;
                }
            })
            .await
            .expect("clangd completion");
            let items = completion["result"]["items"]
                .as_array()
                .or_else(|| completion["result"].as_array())
                .expect("clangd completion items");
            assert!(
                items.iter().any(|item| item["label"]
                    .as_str()
                    .is_some_and(|label| label.trim_start().starts_with("greeting"))),
                "{completion}"
            );
            drop(server);
        }
    }
    if std::env::var_os("AUTONOMICS_DEVCONTAINER_TEST_INSTALL_GO").is_some() {
        let gopls = std::env::var("AUTONOMICS_DEVCONTAINER_TEST_GOPLS_BIN")
            .expect("gopls binary for isolated test");
        let goroot = std::env::var("AUTONOMICS_DEVCONTAINER_TEST_GO_ROOT")
            .expect("Go toolchain for isolated test");
        let copied = timeout(
            Duration::from_secs(180),
            tokio::process::Command::new("cp")
                .arg("-R")
                .arg(goroot)
                .arg(workspace.path().join(".test-go"))
                .stdin(Stdio::null())
                .kill_on_drop(true)
                .output(),
        )
        .await
        .expect("Go copy timeout")
        .expect("copy Go toolchain");
        assert!(
            copied.status.success(),
            "{}",
            String::from_utf8_lossy(&copied.stderr)
        );
        let tools = workspace.path().join(".test-tools");
        std::fs::create_dir(&tools).unwrap();
        std::fs::copy(gopls, tools.join("gopls")).unwrap();
        std::fs::write(
            workspace.path().join("go.mod"),
            "module example.com/container-test\n\ngo 1.25\n",
        )
        .unwrap();
        let source = "package main\nvar greeting = 1\nfunc main() { gree }\n";
        std::fs::write(workspace.path().join("example.go"), source).unwrap();
        let mut go_env = remote_env.clone();
        let path = go_env
            .get("PATH")
            .cloned()
            .unwrap_or_else(|| "/usr/local/bin:/usr/bin:/bin".into());
        go_env.insert(
            "PATH".into(),
            format!("/work/.test-tools:/work/.test-go/bin:{path}"),
        );
        go_env.insert("GOROOT".into(), "/work/.test-go".into());
        go_env.insert("GOCACHE".into(), "/tmp/autonomics-gocache".into());
        go_env.insert("GOMODCACHE".into(), "/tmp/autonomics-gomod".into());
        go_env.insert("GOPROXY".into(), "off".into());
        let mut server =
            super::super::devlsp::LanguageServer::start(&config, &first, "go", &go_env, None)
                .await
                .expect("start gopls in container");
        server.send(json!({"jsonrpc":"2.0","id":40,"method":"initialize","params":{"processId":null,"rootUri":"file:///work","capabilities":{}}})).await.unwrap();
        let mut cursor = 0;
        let initialized = timeout(Duration::from_secs(30), async {
            loop {
                let (messages, next, _, _) = server.poll(cursor).unwrap();
                cursor = next;
                if let Some(message) = messages.into_iter().find(|message| message["id"] == 40) {
                    break message;
                }
                sleep(Duration::from_millis(50)).await;
            }
        })
        .await
        .expect("gopls initialize");
        assert!(
            initialized["result"]["capabilities"]["completionProvider"].is_object(),
            "{initialized}"
        );
        server
            .send(json!({"jsonrpc":"2.0","method":"initialized","params":{}}))
            .await
            .unwrap();
        server.send(json!({"jsonrpc":"2.0","method":"textDocument/didOpen","params":{"textDocument":{"uri":"file:///work/example.go","languageId":"go","version":1,"text":source}}})).await.unwrap();
        server.send(json!({"jsonrpc":"2.0","id":41,"method":"textDocument/completion","params":{"textDocument":{"uri":"file:///work/example.go"},"position":{"line":2,"character":18}}})).await.unwrap();
        let completion = timeout(Duration::from_secs(60), async {
            loop {
                let (messages, next, _, _) = server.poll(cursor).unwrap();
                cursor = next;
                if let Some(message) = messages.into_iter().find(|message| message["id"] == 41) {
                    break message;
                }
                sleep(Duration::from_millis(50)).await;
            }
        })
        .await
        .expect("gopls completion");
        let items = completion["result"]["items"]
            .as_array()
            .or_else(|| completion["result"].as_array())
            .expect("gopls completion items");
        assert!(
            items.iter().any(|item| item["label"] == "greeting"),
            "{completion}"
        );
        drop(server);
    }
    if std::env::var_os("AUTONOMICS_DEVCONTAINER_TEST_INSTALL_LSP").is_some() {
        let install = timeout(Duration::from_secs(180), config.async_command().args(["exec", "--workdir=/work", &first, "/bin/sh", "-c", "python3 -m pip install --disable-pip-version-check --no-cache-dir --target /tmp/lspdeps python-lsp-server==1.12.2 >/dev/null"]).stdin(Stdio::null()).output()).await.expect("install timeout").expect("install test language server");
        assert!(
            install.status.success(),
            "{}",
            String::from_utf8_lossy(&install.stderr)
        );
        let shim = manager.exec(&config, "test-project", "mkdir -p /work/.venv/bin; printf '#!/bin/sh\\nPYTHONPATH=/tmp/lspdeps exec python3 -m pylsp\\n' > /work/.venv/bin/pylsp; chmod +x /work/.venv/bin/pylsp", &[], &[]).await;
        assert!(
            matches!(shim, Ok(output) if output.status.success()),
            "create test virtual environment shim"
        );
        let listing = match manager
            .exec(&config, "test-project", LIST_SCRIPT, &[], &[])
            .await
        {
            Ok(output) => output,
            Err(_) => panic!("list files"),
        };
        assert!(
            !listing
                .stdout
                .split(|byte| *byte == 0)
                .any(|file| file.starts_with(b"./.venv/"))
        );
        let mut server = super::super::devlsp::LanguageServer::start(
            &config,
            &first,
            "python",
            &remote_env,
            None,
        )
        .await
        .expect("start Python language server");
        server.send(json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"processId":null,"rootUri":"file:///work","capabilities":{}}})).await.unwrap();
        let mut cursor = 0;
        let initialized = timeout(Duration::from_secs(20), async {
            loop {
                let (messages, next, _, _) = server.poll(cursor).unwrap();
                cursor = next;
                if let Some(message) = messages.into_iter().find(|message| message["id"] == 1) {
                    break message;
                }
                sleep(Duration::from_millis(50)).await;
            }
        })
        .await
        .expect("Python initialization response");
        assert!(
            initialized["result"]["capabilities"].is_object(),
            "{initialized}"
        );
        server
            .send(json!({"jsonrpc":"2.0","method":"initialized","params":{}}))
            .await
            .unwrap();
        server.send(json!({"jsonrpc":"2.0","method":"textDocument/didOpen","params":{"textDocument":{"uri":"file:///work/test.py","languageId":"python","version":1,"text":"pr"}}})).await.unwrap();
        server.send(json!({"jsonrpc":"2.0","method":"textDocument/didChange","params":{"textDocument":{"uri":"file:///work/test.py","version":2},"contentChanges":[{"text":"pri"}]}})).await.unwrap();
        server.send(json!({"jsonrpc":"2.0","id":2,"method":"textDocument/completion","params":{"textDocument":{"uri":"file:///work/test.py"},"position":{"line":0,"character":3}}})).await.unwrap();
        let completion = timeout(Duration::from_secs(20), async {
            loop {
                let (messages, next, _, _) = server.poll(cursor).unwrap();
                cursor = next;
                if let Some(message) = messages.into_iter().find(|message| message["id"] == 2) {
                    break message;
                }
                sleep(Duration::from_millis(50)).await;
            }
        })
        .await
        .expect("Python completion response");
        let items = completion["result"]["items"]
            .as_array()
            .or_else(|| completion["result"].as_array())
            .expect("completion items");
        assert!(
            items.iter().any(|item| item["insertText"] == "print"
                || item["label"]
                    .as_str()
                    .is_some_and(|label| label.starts_with("print"))),
            "{completion}"
        );
        let created = manager
            .exec(
                &config,
                "test-project",
                "printf 'value = 1\\nprint(value)\\n' > /work/test.py",
                &[],
                &[],
            )
            .await;
        assert!(matches!(created, Ok(output) if output.status.success()));
        server.send(json!({"jsonrpc":"2.0","method":"textDocument/didChange","params":{"textDocument":{"uri":"file:///work/test.py","version":3},"contentChanges":[{"text":"value = 1\nprint(value)\n"}]}})).await.unwrap();
        server.send(json!({"jsonrpc":"2.0","id":3,"method":"textDocument/rename","params":{"textDocument":{"uri":"file:///work/test.py"},"position":{"line":0,"character":1},"newName":"renamed"}})).await.unwrap();
        let rename = timeout(Duration::from_secs(20), async {
            loop {
                let (messages, next, _, _) = server.poll(cursor).unwrap();
                cursor = next;
                if let Some(message) = messages.into_iter().find(|message| message["id"] == 3) {
                    break message;
                }
                sleep(Duration::from_millis(50)).await;
            }
        })
        .await
        .expect("Python rename response");
        let edits = rename["result"]["documentChanges"][0]["edits"]
            .as_array()
            .expect("rename edits");
        assert!(
            edits
                .iter()
                .filter_map(|edit| edit["newText"].as_str())
                .any(|text| text.matches("renamed").count() >= 2),
            "{rename}"
        );
        drop(server);
    }
    if std::env::var_os("AUTONOMICS_DEVCONTAINER_TEST_INSTALL_DEBUGPY").is_some() {
        let install = timeout(Duration::from_secs(180), config.async_command().args(["exec", "--workdir=/work", &first, "/bin/sh", "-c", "python3 -m venv /work/.venv && /work/.venv/bin/python -m pip install --disable-pip-version-check --no-cache-dir debugpy==1.8.21 >/dev/null && printf 'x = 1\\nx += 1\\nprint(x)\\n' > /work/debug.py"]).stdin(Stdio::null()).output()).await.expect("debugpy install timeout").expect("install debugpy");
        assert!(
            install.status.success(),
            "{}",
            String::from_utf8_lossy(&install.stderr)
        );
        let mut adapter = super::super::devlsp::LanguageServer::start_debug(
            &config,
            &first,
            "python",
            &remote_env,
            None,
        )
        .await
        .expect("start debug adapter");
        let mut offset = 0;
        let mut pending = Vec::new();
        adapter.send(json!({"seq":1,"type":"request","command":"initialize","arguments":{"clientID":"autonomics","adapterID":"python","pathFormat":"path","linesStartAt1":true,"columnsStartAt1":true,"supportsRunInTerminalRequest":false}})).await.unwrap();
        let initialized = debug_message(&mut adapter, &mut offset, &mut pending, |m| {
            m["type"] == "response" && m["request_seq"] == 1
        })
        .await;
        assert_eq!(initialized["success"], true, "{initialized}");
        adapter.send(json!({"seq":2,"type":"request","command":"launch","arguments":{"name":"test","type":"python","request":"launch","program":"/work/debug.py","cwd":"/work","console":"internalConsole","justMyCode":true}})).await.unwrap();
        debug_message(&mut adapter, &mut offset, &mut pending, |m| {
            m["type"] == "event" && m["event"] == "initialized"
        })
        .await;
        adapter.send(json!({"seq":3,"type":"request","command":"setBreakpoints","arguments":{"source":{"path":"/work/debug.py"},"breakpoints":[{"line":2}]}})).await.unwrap();
        let breakpoints = debug_message(&mut adapter, &mut offset, &mut pending, |m| {
            m["type"] == "response" && m["request_seq"] == 3
        })
        .await;
        assert_eq!(
            breakpoints["body"]["breakpoints"][0]["verified"], true,
            "{breakpoints}"
        );
        adapter
            .send(json!({"seq":4,"type":"request","command":"configurationDone","arguments":{}}))
            .await
            .unwrap();
        let stopped = debug_message(&mut adapter, &mut offset, &mut pending, |m| {
            m["type"] == "event" && m["event"] == "stopped"
        })
        .await;
        assert_eq!(stopped["body"]["reason"], "breakpoint");
        let thread = stopped["body"]["threadId"]
            .as_i64()
            .expect("stopped thread");
        adapter.send(json!({"seq":5,"type":"request","command":"stackTrace","arguments":{"threadId":thread}})).await.unwrap();
        let stack = debug_message(&mut adapter, &mut offset, &mut pending, |m| {
            m["type"] == "response" && m["request_seq"] == 5
        })
        .await;
        assert_eq!(stack["body"]["stackFrames"][0]["line"], 2, "{stack}");
        let frame = stack["body"]["stackFrames"][0]["id"]
            .as_i64()
            .expect("stack frame");
        adapter
            .send(
                json!({"seq":6,"type":"request","command":"scopes","arguments":{"frameId":frame}}),
            )
            .await
            .unwrap();
        let scopes = debug_message(&mut adapter, &mut offset, &mut pending, |m| {
            m["type"] == "response" && m["request_seq"] == 6
        })
        .await;
        let locals = scopes["body"]["scopes"]
            .as_array()
            .unwrap()
            .iter()
            .find(|scope| scope["name"] == "Locals")
            .expect("locals scope")["variablesReference"]
            .as_i64()
            .unwrap();
        adapter.send(json!({"seq":7,"type":"request","command":"variables","arguments":{"variablesReference":locals}})).await.unwrap();
        let variables = debug_message(&mut adapter, &mut offset, &mut pending, |m| {
            m["type"] == "response" && m["request_seq"] == 7
        })
        .await;
        assert!(
            variables["body"]["variables"]
                .as_array()
                .unwrap()
                .iter()
                .any(|variable| variable["name"] == "x" && variable["value"] == "1"),
            "{variables}"
        );
        adapter.send(json!({"seq":8,"type":"request","command":"continue","arguments":{"threadId":thread}})).await.unwrap();
        let continued = debug_message(&mut adapter, &mut offset, &mut pending, |m| {
            m["type"] == "response" && m["request_seq"] == 8
        })
        .await;
        assert_eq!(continued["success"], true, "{continued}");
        debug_message(&mut adapter, &mut offset, &mut pending, |m| {
            m["type"] == "event" && m["event"] == "terminated"
        })
        .await;
        drop(adapter);
    }
    manager.close("test-project").await;
    assert!(
        manager
            .exec(&config, "test-project", "exit 0", &[], &[])
            .await
            .is_err()
    );
    assert!(!running(&config, &first).await);
    let manager = DevContainers::default();
    let container = match manager
        .connect(&config, "test-project", &image, workspace.path())
        .await
    {
        Ok(name) => name,
        Err(_) => panic!("container did not start"),
    };
    drop(manager);
    timeout(Duration::from_secs(10), async {
        loop {
            let exists = config
                .async_command()
                .args(["container", "exists", &container])
                .status()
                .await
                .expect("container exists check");
            if !exists.success() {
                break;
            }
            sleep(Duration::from_millis(100)).await;
        }
    })
    .await
    .expect("dropping session must remove its container");
    let failing_workspace = tempfile::tempdir().unwrap();
    std::fs::write(
        failing_workspace.path().join(".devcontainer.json"),
        r#"{"postCreateCommand":"exit 7"}"#,
    )
    .unwrap();
    let manager = DevContainers::default();
    assert!(
        manager
            .connect(&config, "failed-project", &image, failing_workspace.path())
            .await
            .is_err()
    );
    assert!(manager.active("failed-project").await.is_err());
}
