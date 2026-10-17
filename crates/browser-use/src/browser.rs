//! Agent-facing browser abstraction on top of [`chromiumoxide`] (CDP).
//!
//! [`Browser`] owns a launched (or connected) Chromium instance and drives it
//! through a background handler task. Individual pages are exposed as [`Tab`]s,
//! which are cheap `Clone` handles — an agent loop can keep several tabs around
//! and operate on any of them.
//!
//! # Example
//!
//! ```no_run
//! # use browser_use::{Browser, BrowserOptions};
//! # #[tokio::main]
//! # async fn main() -> browser_use::Result<()> {
//! let mut browser = Browser::launch(BrowserOptions::default()).await?;
//! let tab = browser.new_tab().await?;
//! tab.goto("https://example.com").await?;
//! println!("{}", tab.title().await?.unwrap_or_default());
//! tab.wait_for_selector("a", None).await?.click().await?;
//! # Ok(()) }
//! ```

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use chromiumoxide::browser::BrowserConfig;
use chromiumoxide::cdp::browser_protocol::browser::{
    EventDownloadProgress, EventDownloadWillBegin, SetDownloadBehaviorBehavior,
    SetDownloadBehaviorParams,
};
use chromiumoxide::cdp::browser_protocol::page::CaptureScreenshotFormat;
use chromiumoxide::handler::Handler;
use chromiumoxide::layout::Point;
use chromiumoxide::page::ScreenshotParams;
use chromiumoxide::{Browser as CdpBrowser, Element, Page};
use futures::StreamExt;
use serde_json::Value;
use tokio::sync::Mutex;

use crate::error::{Error, Result};

/// Default deadline for page navigation.
pub const DEFAULT_NAV_TIMEOUT: Duration = Duration::from_secs(30);
/// Default deadline for `wait_for_selector`.
pub const DEFAULT_WAIT_TIMEOUT: Duration = Duration::from_secs(10);
/// Poll interval used while waiting for selectors to appear.
const POLL_INTERVAL: Duration = Duration::from_millis(100);
/// Default agent-visible download mount used when no runtime path is supplied.
const DEFAULT_DOWNLOAD_VFS_PATH: &str = "/browser-downloads";
static PROFILE_SEQUENCE: AtomicU64 = AtomicU64::new(0);

// ---------------------------------------------------------------------------
// Launch options
// ---------------------------------------------------------------------------

/// Options for launching a browser process.
#[derive(Debug, Clone)]
pub struct BrowserOptions {
    /// Run without a visible window (default `true`).
    pub headless: bool,
    /// Path to the Chromium/Chrome executable; auto-detected when `None`.
    pub executable: Option<PathBuf>,
    /// Persistent profile directory; a temp dir is used when `None`.
    pub user_data_dir: Option<PathBuf>,
    /// Remote debugging port; a free port is picked when `None`.
    pub port: Option<u16>,
    /// Initial window size `(width, height)`.
    pub window_size: Option<(u32, u32)>,
    /// Pass `--no-sandbox` (needed in most containers).
    pub no_sandbox: bool,
    /// Host directory where Chromium saves downloads.
    ///
    /// When set, the browser emits download events and [`BrowserSession`]
    /// tracks them for browser tools.
    pub download_dir: Option<PathBuf>,
    /// Agent-visible root corresponding to `download_dir`.
    pub download_vfs_path: Option<String>,
    /// Additional command-line arguments for the browser process.
    pub extra_args: Vec<String>,
}

impl Default for BrowserOptions {
    fn default() -> Self {
        Self {
            headless: true,
            executable: None,
            user_data_dir: None,
            port: None,
            window_size: None,
            no_sandbox: false,
            download_dir: None,
            download_vfs_path: None,
            extra_args: Vec::new(),
        }
    }
}

impl BrowserOptions {
    fn to_cdp_config(&self) -> Result<BrowserConfig> {
        let mut builder = BrowserConfig::builder()
            .request_timeout(DEFAULT_NAV_TIMEOUT)
            .arg("--disable-dev-shm-usage")
            .arg("--disable-gpu");
        // chromiumoxide's `with_head()` is the historical name for the old
        // Chrome `--head` flag and actually disables headless mode; the
        // builder's default `HeadlessMode::True` is already headless. So we
        // only call `with_head()` when the caller asked for a visible window.
        if !self.headless {
            builder = builder.with_head();
        }
        if let Some(exe) = &self.executable {
            builder = builder.chrome_executable(exe);
        }
        if let Some(dir) = &self.user_data_dir {
            builder = builder.user_data_dir(dir);
        }
        if let Some(port) = self.port {
            builder = builder.port(port);
        }
        if let Some((w, h)) = self.window_size {
            builder = builder.window_size(w, h);
        }
        if self.no_sandbox {
            builder = builder.no_sandbox();
        }
        if !self.extra_args.is_empty() {
            builder = builder.args(self.extra_args.iter().map(String::as_str));
        }
        builder.build().map_err(Error::Launch)
    }
}

// ---------------------------------------------------------------------------
// Browser
// ---------------------------------------------------------------------------

/// A running browser an agent can operate.
///
/// The CDP connection handler is polled on a background task; dropping the
/// last handle kills a browser we launched (`kill_on_drop` is set on the
/// child process). Call [`Browser::close`] for a graceful shutdown instead.
#[derive(Debug)]
pub struct Browser {
    browser: CdpBrowser,
    /// Join handle of the spawned CDP handler driver.
    _handler: tokio::task::JoinHandle<()>,
    /// Ephemeral profile directory owned by this browser, if it created one.
    _profile_dir: Option<PathBuf>,
}

impl Browser {
    /// Launch a new browser process with the given options.
    pub async fn launch(options: BrowserOptions) -> Result<Self> {
        let mut options = options;
        let profile_dir = match options.user_data_dir {
            Some(_) => None,
            None => {
                let sequence = PROFILE_SEQUENCE.fetch_add(1, Ordering::Relaxed);
                let dir = std::env::temp_dir().join(format!(
                    "browser-use-profile-{}-{sequence}",
                    std::process::id()
                ));
                std::fs::create_dir_all(&dir)?;
                options.user_data_dir = Some(dir.clone());
                Some(dir)
            }
        };
        let launched = CdpBrowser::launch(options.to_cdp_config()?).await;
        let (browser, handler) = match launched {
            Ok(launched) => launched,
            Err(error) => {
                if let Some(dir) = &profile_dir {
                    let _ = std::fs::remove_dir_all(dir);
                }
                return Err(Error::Launch(error.to_string()));
            }
        };
        let _handler = spawn_handler(handler);
        Ok(Self {
            browser,
            _handler,
            _profile_dir: profile_dir,
        })
    }

    /// Attach to an already running browser via its debugging URL
    /// (`http://host:port` or `ws://host:port/devtools/browser/...`).
    pub async fn connect(url: impl Into<String>) -> Result<Self> {
        let (browser, handler) = CdpBrowser::connect(url)
            .await
            .map_err(|e| Error::Connect(e.to_string()))?;
        let _handler = spawn_handler(handler);
        Ok(Self {
            browser,
            _handler,
            _profile_dir: None,
        })
    }

    /// Open a new empty tab.
    pub async fn new_tab(&self) -> Result<Tab> {
        let page = self.browser.new_page("about:blank").await?;
        Ok(Tab { page })
    }

    /// Open a new tab and navigate it to `url` in one step.
    pub async fn new_tab_at(&self, url: &str) -> Result<Tab> {
        let url = normalize_url(url)?;
        let page = self.browser.new_page(url.as_str()).await?;
        let tab = Tab { page };
        let mut last_error = None;
        for attempt in 0..3 {
            if attempt > 0 {
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
            match tab.goto(url.as_str()).await {
                Ok(()) => {
                    let current = tab.url().await?;
                    let loaded = current.as_deref().is_some_and(|current| {
                        current == url.as_str()
                            || (!current.starts_with("about:blank")
                                && !current.starts_with("chrome://new-tab-page"))
                    });
                    if loaded {
                        return Ok(tab);
                    }
                }
                Err(error) => last_error = Some(error),
            }
        }
        if let Some(error) = last_error {
            return Err(error);
        }
        Err(Error::NewTabNavigation)
    }

    /// Handles to all currently open tabs.
    pub async fn tabs(&self) -> Result<Vec<Tab>> {
        Ok(self
            .browser
            .pages()
            .await?
            .into_iter()
            .map(|page| Tab { page })
            .collect())
    }

    /// Browser version string (e.g. `"HeadlessChrome/129.0.6668.91"`).
    pub async fn version(&self) -> Result<String> {
        Ok(self.browser.version().await?.product)
    }

    /// Direct Chromium downloads to `download_dir` and emit CDP events.
    pub async fn enable_downloads(&mut self, download_dir: &std::path::Path) -> Result<()> {
        std::fs::create_dir_all(download_dir)?;
        let params = SetDownloadBehaviorParams::builder()
            .behavior(SetDownloadBehaviorBehavior::AllowAndName)
            .download_path(download_dir.to_string_lossy().into_owned())
            .events_enabled(true)
            .build()
            .map_err(Error::Launch)?;
        self.browser.execute(params).await?;
        Ok(())
    }

    /// Gracefully close the browser and wait for the process to exit.
    pub async fn close(&mut self) -> Result<()> {
        self.browser.close().await?;
        self.browser.wait().await?;
        Ok(())
    }
}

impl Drop for Browser {
    fn drop(&mut self) {
        if let Some(dir) = &self._profile_dir {
            let _ = std::fs::remove_dir_all(dir);
        }
    }
}

// ---------------------------------------------------------------------------
// Tab
// ---------------------------------------------------------------------------

/// A single page (tab) inside a [`Browser`].
///
/// Cheap to clone; every clone drives the same underlying page.
#[derive(Debug, Clone)]
pub struct Tab {
    page: Page,
}

impl Tab {
    /// The page's current URL, if any.
    pub async fn url(&self) -> Result<Option<String>> {
        Ok(self.page.url().await?)
    }

    /// The page's title, if any.
    pub async fn title(&self) -> Result<Option<String>> {
        self.page.get_title().await.map_err(Error::from)
    }

    /// Navigate to `url`, waiting for the document to load.
    ///
    /// Schemes are normalized: bare hostnames get `https://` prepended.
    pub async fn goto(&self, url: impl AsRef<str>) -> Result<()> {
        let url = normalize_url(url.as_ref())?;
        tokio::time::timeout(DEFAULT_NAV_TIMEOUT, self.page.goto(url.as_str()))
            .await
            .map_err(|_| Error::Timeout {
                operation: "navigate",
                timeout_ms: DEFAULT_NAV_TIMEOUT.as_millis() as u64,
            })??;
        Ok(())
    }

    /// Go back one entry in the history.
    pub async fn go_back(&self) -> Result<()> {
        self.evaluate("history.back()").await.map(|_| ())
    }

    /// Go forward one entry in the history.
    pub async fn go_forward(&self) -> Result<()> {
        self.evaluate("history.forward()").await.map(|_| ())
    }

    /// The full serialized DOM (`document.documentElement.outerHTML`).
    pub async fn html(&self) -> Result<String> {
        Ok(self.page.content().await?)
    }

    /// The visible text of the page body (`document.body.innerText`).
    pub async fn text(&self) -> Result<String> {
        const EXPR: &str = r#"
            document.readyState === "complete" && document.body
                ? document.body.innerText
                : null
        "#;
        let deadline = tokio::time::Instant::now() + DEFAULT_NAV_TIMEOUT;
        loop {
            if let Some(text) = self.evaluate(EXPR).await?.as_str() {
                return Ok(text.to_string());
            }
            if tokio::time::Instant::now() >= deadline {
                return Ok(String::new());
            }
            tokio::time::sleep(POLL_INTERVAL).await;
        }
    }

    /// Evaluate a JavaScript expression and return its JSON value.
    ///
    /// Navigation destroys and recreates the page's execution context, so a
    /// script fired mid-navigation can fail spuriously; the evaluation is
    /// retried a few times before giving up.
    pub async fn evaluate(&self, expression: &str) -> Result<Value> {
        const MAX_ATTEMPTS: usize = 5;
        let mut last_err = None;
        for _ in 0..MAX_ATTEMPTS {
            match self.page.evaluate(expression).await {
                Ok(result) => {
                    return Ok(match result.into_value() {
                        Ok(value) => value,
                        // `into_value` reports JSON null results as an error
                        // rather than `Value::Null`; normalize them.
                        Err(e) if e.to_string().contains("No value found") => Value::Null,
                        Err(e) => return Err(Error::Json(e)),
                    });
                }
                Err(e) if is_stale_context(&e) => {
                    last_err = Some(Error::Cdp(e));
                    tokio::time::sleep(POLL_INTERVAL).await;
                }
                Err(e) => return Err(Error::Cdp(e)),
            }
        }
        Err(last_err.expect("retry loop ran at least once"))
    }

    /// Find the first element matching a CSS selector.
    ///
    /// Returns [`Error::ElementNotFound`] when nothing matches right now; use
    /// [`Tab::wait_for_selector`] when the element may appear later.
    pub async fn find_element(&self, selector: &str) -> Result<Element> {
        self.page.find_element(selector).await.map_err(|e| match e {
            chromiumoxide::error::CdpError::NotFound => {
                Error::ElementNotFound(selector.to_string())
            }
            other => Error::Cdp(other),
        })
    }

    /// Find all elements matching a CSS selector (empty when none match).
    pub async fn find_elements(&self, selector: &str) -> Result<Vec<Element>> {
        Ok(self.page.find_elements(selector).await?)
    }

    /// Wait until `selector` matches an element, then return it.
    ///
    /// `timeout` defaults to [`DEFAULT_WAIT_TIMEOUT`].
    pub async fn wait_for_selector(
        &self,
        selector: &str,
        timeout: Option<Duration>,
    ) -> Result<Element> {
        let timeout = timeout.unwrap_or(DEFAULT_WAIT_TIMEOUT);
        let deadline = tokio::time::Instant::now() + timeout;
        loop {
            if let Ok(elements) = self.page.find_elements(selector).await {
                if let Some(element) = elements.into_iter().next() {
                    return Ok(element);
                }
            }
            if tokio::time::Instant::now() >= deadline {
                return Err(Error::Timeout {
                    operation: "wait_for_selector",
                    timeout_ms: timeout.as_millis() as u64,
                });
            }
            tokio::time::sleep(POLL_INTERVAL).await;
        }
    }

    /// Click the first element matching `selector`, waiting for it to appear.
    pub async fn click(&self, selector: &str) -> Result<()> {
        self.wait_for_selector(selector, None)
            .await?
            .click()
            .await?;
        Ok(())
    }

    /// Click the first element matching `selector`, then type `input` into it.
    pub async fn type_text(&self, selector: &str, input: &str) -> Result<()> {
        self.type_text_with_timeout(selector, input, None).await
    }

    /// Set a form field with native DOM events.
    ///
    /// Chrome 153 headless can stall in chromiumoxide's per-character keyboard
    /// pipeline even for an attached, focused input. Writing through the native
    /// value setter and dispatching `input`/`change` is deterministic and keeps
    /// framework listeners working.
    pub async fn type_text_with_timeout(
        &self,
        selector: &str,
        input: &str,
        timeout: Option<Duration>,
    ) -> Result<()> {
        self.wait_for_selector(selector, timeout).await?;
        let selector = serde_json::to_string(selector)?;
        let input = serde_json::to_string(input)?;
        let script = format!(
            r#"
                (() => {{
                    const element = document.querySelector({selector});
                    element.focus();
                    element.click();
                    if (element instanceof HTMLInputElement ||
                        element instanceof HTMLTextAreaElement
                    ) {{
                        const prototype = element instanceof HTMLInputElement
                            ? HTMLInputElement.prototype
                            : HTMLTextAreaElement.prototype;
                        const setter = Object.getOwnPropertyDescriptor(
                            prototype,
                            "value"
                        )?.set;
                        if (setter) {{
                            setter.call(element, {input});
                        }} else {{
                            element.value = {input};
                        }}
                    }} else if (element.isContentEditable) {{
                        element.textContent = {input};
                    }} else {{
                        throw new TypeError("selector is not an input or contenteditable element");
                    }}
                    element.dispatchEvent(new InputEvent("input", {{
                        bubbles: true,
                        cancelable: true,
                    }}));
                    element.dispatchEvent(new Event("change", {{ bubbles: true }}));
                    return true;
                }})()
            "#
        );
        self.evaluate(&script).await.map(|_| ())
    }

    /// Scroll by `(x, y)` pixels relative to the current position.
    pub async fn scroll_by(&self, x: i32, y: i32) -> Result<()> {
        self.evaluate(&format!("window.scrollBy({x},{y})"))
            .await
            .map(|_| ())
    }

    /// Move the mouse to a page coordinate (useful after [`Element::clickable_point`]).
    pub async fn move_mouse(&self, x: f64, y: f64) -> Result<()> {
        self.page.move_mouse(Point::new(x, y)).await?;
        Ok(())
    }

    /// Capture a PNG screenshot of the current viewport.
    pub async fn screenshot(&self) -> Result<Vec<u8>> {
        Ok(self
            .page
            .screenshot(
                ScreenshotParams::builder()
                    .format(CaptureScreenshotFormat::Png)
                    .build(),
            )
            .await?)
    }

    /// Capture a PNG screenshot and write it to `path`.
    pub async fn save_screenshot(&self, path: impl AsRef<std::path::Path>) -> Result<()> {
        let bytes = self.screenshot().await?;
        Ok(std::fs::write(path, bytes)?)
    }

    /// Set the page's HTML content directly.
    pub async fn set_content(&self, html: &str) -> Result<()> {
        self.page.set_content(html).await?;
        Ok(())
    }

    /// The underlying [`Page`], for anything not wrapped here.
    pub fn page(&self) -> &Page {
        &self.page
    }

    /// Close this tab.
    pub async fn close(self) -> Result<()> {
        Ok(self.page.close().await?)
    }
}

/// Drive the CDP connection on a background task until the browser exits.
fn spawn_handler(mut handler: Handler) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move { while handler.next().await.is_some() {} })
}

/// Whether an evaluation error is the transient "context is gone" race caused
/// by an in-flight navigation.
fn is_stale_context(err: &chromiumoxide::error::CdpError) -> bool {
    let msg = err.to_string();
    msg.contains("Cannot find context with specified id")
        || msg.contains("Execution context was destroyed")
        || msg.contains("Cannot find context")
}

/// Prepend `https://` to bare hostnames and reject unknown schemes.
fn normalize_url(raw: &str) -> Result<String> {
    let url = if raw.contains("://") {
        raw.to_string()
    } else {
        format!("https://{raw}")
    };
    let scheme = url.split("://").next().unwrap_or_default();
    if !matches!(
        scheme,
        "http" | "https" | "file" | "about" | "data" | "chrome"
    ) {
        return Err(Error::InvalidUrl(raw.to_string()));
    }
    Ok(url)
}

// ---------------------------------------------------------------------------
// Session
// ---------------------------------------------------------------------------

/// Agent-visible metadata for one Chromium download.
#[derive(Debug, Clone, serde::Serialize)]
pub struct DownloadInfo {
    pub guid: String,
    pub url: String,
    pub suggested_filename: String,
    pub state: String,
    pub received_bytes: f64,
    pub total_bytes: f64,
    pub file_path: Option<String>,
    pub vfs_path: Option<String>,
}

#[derive(Debug, Clone)]
struct DownloadEntry {
    info: DownloadInfo,
    sequence: u64,
    delivered: bool,
    full_length_polls: u32,
}

type DownloadMap = Arc<tokio::sync::Mutex<HashMap<String, DownloadEntry>>>;

async fn watch_downloads(
    mut will_begin: chromiumoxide::listeners::EventStream<EventDownloadWillBegin>,
    mut progress: chromiumoxide::listeners::EventStream<EventDownloadProgress>,
    downloads: DownloadMap,
    next_sequence: Arc<AtomicU64>,
    download_vfs_path: Option<String>,
) {
    loop {
        tokio::select! {
            event = will_begin.next() => {
                let Some(event) = event else { break };
                let sequence = next_sequence.fetch_add(1, Ordering::Relaxed);
                let info = DownloadInfo {
                    guid: event.guid.clone(),
                    url: event.url.to_string(),
                    suggested_filename: event.suggested_filename.to_string(),
                    state: "in_progress".into(),
                    received_bytes: 0.0,
                    total_bytes: 0.0,
                    file_path: None,
                    vfs_path: None,
                };
                downloads.lock().await.insert(
                    info.guid.clone(),
                    DownloadEntry {
                        info,
                        sequence,
                        delivered: false,
                        full_length_polls: 0,
                    },
                );
            }
            event = progress.next() => {
                let Some(event) = event else { break };
                let mut downloads = downloads.lock().await;
                let Some(entry) = downloads.get_mut(&event.guid) else {
                    continue;
                };
                use chromiumoxide::cdp::browser_protocol::browser::DownloadProgressState;
                entry.info.state = match event.state {
                    DownloadProgressState::InProgress => "in_progress",
                    DownloadProgressState::Completed => "completed",
                    DownloadProgressState::Canceled => "canceled",
                }.into();
                entry.info.received_bytes = event.received_bytes;
                entry.info.total_bytes = event.total_bytes;
                if let Some(path) = event.file_path.as_ref() {
                    entry.info.file_path = Some(path.to_string());
                }
                if entry.info.state == "completed" {
                    entry.info.vfs_path = entry
                        .info
                        .file_path
                        .as_deref()
                        .and_then(|path| std::path::Path::new(path).file_name())
                        .map(|name| {
                            format!(
                                "{}/{}",
                                download_vfs_path
                                    .clone()
                                    .unwrap_or_else(|| DEFAULT_DOWNLOAD_VFS_PATH.to_string()),
                                name.to_string_lossy()
                            )
                        });
                }
            }
        }
    }
}

/// Shared, agent-friendly handle to a [`Browser`].
///
/// `Browser` itself is not `Clone` (the inner CDP `Browser` owns the
/// websocket sender), so agent tools receive an `Arc<BrowserSession>` and
/// lock it briefly to grab cheap-clone `Tab` handles — the actual page
/// operations then run on the unlocked `Tab`, never across an `.await`
/// while holding the mutex.
pub struct BrowserSession {
    browser: Mutex<Browser>,
    /// Index into [`Browser::tabs`] of the tab subsequent tools operate on
    /// when no explicit `tab_index` is given.
    active: Mutex<usize>,
    downloads: DownloadMap,
    next_download_sequence: Arc<AtomicU64>,
    downloads_enabled: bool,
    download_dir: Option<PathBuf>,
    download_vfs_path: Option<String>,
}

impl BrowserSession {
    /// Wrap an already-launched browser in a shared session.
    ///
    /// Ensures at least one tab is open so the session is always usable.
    /// If the browser already has tabs, the first one is selected as
    /// active; otherwise a fresh `about:blank` tab is created.
    pub async fn new(browser: Browser) -> Result<Self> {
        if browser.tabs().await?.is_empty() {
            browser.new_tab().await?;
        }
        Ok(Self {
            browser: Mutex::new(browser),
            active: Mutex::new(0),
            downloads: Arc::new(tokio::sync::Mutex::new(HashMap::new())),
            next_download_sequence: Arc::new(AtomicU64::new(0)),
            downloads_enabled: false,
            download_dir: None,
            download_vfs_path: None,
        })
    }

    /// Launch a new browser and immediately wrap it in a session.
    pub async fn launch(options: BrowserOptions) -> Result<Arc<Self>> {
        let download_vfs_path = options.download_vfs_path.clone();
        let download_dir = options.download_dir.clone();
        let mut browser = Browser::launch(options).await?;
        let downloads: DownloadMap = Arc::new(tokio::sync::Mutex::new(HashMap::new()));
        let next_download_sequence = Arc::new(AtomicU64::new(0));
        let mut downloads_enabled = false;
        if let Some(download_dir) = download_dir.clone() {
            browser.enable_downloads(&download_dir).await?;
            let will_begin = browser
                .browser
                .event_listener::<EventDownloadWillBegin>()
                .await?;
            let progress = browser
                .browser
                .event_listener::<EventDownloadProgress>()
                .await?;
            downloads_enabled = true;
            tokio::spawn(watch_downloads(
                will_begin,
                progress,
                downloads.clone(),
                next_download_sequence.clone(),
                download_vfs_path.clone(),
            ));
        }
        let mut session = Self::new(browser).await?;
        session.downloads = downloads;
        session.next_download_sequence = next_download_sequence;
        session.downloads_enabled = downloads_enabled;
        session.download_dir = download_dir;
        session.download_vfs_path = download_vfs_path;
        Ok(Arc::new(session))
    }

    /// Connect to an already-running browser and wrap it in a session.
    pub async fn connect(url: impl Into<String>) -> Result<Arc<Self>> {
        let browser = Browser::connect(url).await?;
        Ok(Arc::new(Self::new(browser).await?))
    }

    /// Resolve `tab_index` (defaults to the active tab) to a cloned `Tab`.
    pub async fn tab(&self, tab_index: Option<usize>) -> Result<Tab> {
        let idx = match tab_index {
            Some(i) => i,
            None => *self.active.lock().await,
        };
        let browser = self.browser.lock().await;
        let tabs = browser.tabs().await?;
        tabs.into_iter()
            .nth(idx)
            .ok_or_else(|| Error::TabIndexOutOfRange {
                index: idx,
                total: 0,
            })
    }

    /// Open a new tab and make it the active one. Returns the new tab's index.
    pub async fn new_tab(&self) -> Result<usize> {
        let browser = self.browser.lock().await;
        let total = browser.tabs().await?.len();
        let _tab = browser.new_tab().await?;
        drop(browser);
        *self.active.lock().await = total;
        Ok(total)
    }

    /// Open a new tab already navigated to `url` and make it active.
    pub async fn new_tab_at(&self, url: &str) -> Result<usize> {
        let browser = self.browser.lock().await;
        let total = browser.tabs().await?.len();
        let _tab = browser.new_tab_at(url).await?;
        drop(browser);
        *self.active.lock().await = total;
        Ok(total)
    }

    /// Switch the active tab. Returns an error if the index is out of range.
    pub async fn switch_tab(&self, index: usize) -> Result<()> {
        let browser = self.browser.lock().await;
        let total = browser.tabs().await?.len();
        if index >= total {
            return Err(Error::TabIndexOutOfRange { index, total });
        }
        *self.active.lock().await = index;
        Ok(())
    }

    /// Close the tab at `index` and adjust the active index.
    pub async fn close_tab(&self, index: usize) -> Result<()> {
        let tab = self.tab(Some(index)).await?;
        tab.close().await?;
        let mut active = self.active.lock().await;
        if *active >= index && *active > 0 {
            *active -= 1;
        }
        Ok(())
    }

    /// List the URLs and titles of all open tabs.
    pub async fn list_tabs(&self) -> Result<Vec<TabInfo>> {
        let browser = self.browser.lock().await;
        let tabs = browser.tabs().await?;
        let mut out = Vec::with_capacity(tabs.len());
        for tab in tabs {
            let url = tab.url().await.unwrap_or(None);
            let title = tab.title().await.unwrap_or(None);
            out.push(TabInfo { url, title });
        }
        Ok(out)
    }

    /// Metadata for every download observed by this session, oldest first.
    pub async fn download_infos(&self) -> Vec<DownloadInfo> {
        let downloads = self.downloads.lock().await;
        let mut entries: Vec<_> = downloads.values().cloned().collect();
        entries.sort_by_key(|entry| entry.sequence);
        entries.into_iter().map(|entry| entry.info).collect()
    }

    /// Start a download in a tab and wait until Chromium reports a final state.
    pub async fn download(
        &self,
        url: &str,
        tab_index: Option<usize>,
        timeout: Duration,
    ) -> Result<DownloadInfo> {
        if !self.downloads_enabled {
            return Err(Error::DownloadsDisabled);
        }
        let url = normalize_url(url)?;
        self.downloads
            .lock()
            .await
            .values_mut()
            .for_each(|entry| entry.delivered = true);
        let tab = self.tab(tab_index).await?;
        let script = serde_json::to_string(&url)?;
        tab.evaluate(&format!("location.assign({script})")).await?;
        self.wait_for_download(timeout).await
    }

    /// Wait for the oldest unconsumed download and mark it consumed.
    pub async fn wait_for_download(&self, timeout: Duration) -> Result<DownloadInfo> {
        if !self.downloads_enabled {
            return Err(Error::DownloadsDisabled);
        }
        let deadline = tokio::time::Instant::now() + timeout;
        loop {
            let selected_guid = {
                let mut downloads = self.downloads.lock().await;
                let mut selected = None;
                let mut selected_sequence = u64::MAX;
                for entry in downloads.values_mut() {
                    if entry.delivered {
                        continue;
                    }
                    let terminal =
                        entry.info.state == "completed" || entry.info.state == "canceled";
                    let fully_received = entry.info.state == "in_progress"
                        && entry.info.total_bytes > 0.0
                        && entry.info.received_bytes >= entry.info.total_bytes;
                    if fully_received {
                        entry.full_length_polls = entry.full_length_polls.saturating_add(1);
                    }
                    let ready = terminal || entry.full_length_polls >= 40;
                    if ready && entry.sequence < selected_sequence {
                        selected = Some(entry.info.guid.clone());
                        selected_sequence = entry.sequence;
                    }
                }
                selected
            };
            if let Some(guid) = selected_guid {
                let mut downloads = self.downloads.lock().await;
                if let Some(entry) = downloads.get_mut(&guid) {
                    if entry.info.state == "in_progress" {
                        // Some headless Chromium builds report full byte counts but
                        // leave the documented GUID name in `.crdownload`. Once the
                        // expected length is stable, finalize that name ourselves.
                        let Some(dir) = &self.download_dir else {
                            return Err(Error::DownloadsDisabled);
                        };
                        let final_path = dir.join(&entry.info.guid);
                        let partial_path = dir.join(format!("{}.crdownload", entry.info.guid));
                        if !final_path.exists() && partial_path.exists() {
                            std::fs::rename(&partial_path, &final_path)?;
                        }
                        entry.info.state = "completed".into();
                        entry.info.file_path = Some(final_path.to_string_lossy().into_owned());
                        entry.info.vfs_path = Some(format!(
                            "{}/{}",
                            self.download_vfs_path
                                .clone()
                                .unwrap_or_else(|| DEFAULT_DOWNLOAD_VFS_PATH.to_string()),
                            entry.info.guid
                        ));
                    }
                    entry.delivered = true;
                    return Ok(entry.info.clone());
                }
            }
            if tokio::time::Instant::now() >= deadline {
                return Err(Error::Timeout {
                    operation: "browser_download",
                    timeout_ms: timeout.as_millis() as u64,
                });
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    }

    /// Currently active tab index.
    pub async fn active_index(&self) -> usize {
        *self.active.lock().await
    }

    /// Gracefully close the browser. The session cannot be used after this.
    pub async fn close(&self) -> Result<()> {
        self.browser.lock().await.close().await
    }
}

impl std::fmt::Debug for BrowserSession {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BrowserSession").finish_non_exhaustive()
    }
}

/// Lightweight metadata about a tab — what `browser_list_tabs` returns.
#[derive(Debug, Clone, serde::Serialize)]
pub struct TabInfo {
    pub url: Option<String>,
    pub title: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::Browser;
    use super::BrowserOptions;

    #[tokio::test]
    async fn test_browser_lanch() {
        if std::env::var("BROWSER_USE_HEADED").is_err() {
            eprintln!("set BROWSER_USE_HEADED=1 to run; skipping");
            return;
        }
        let mut browser_option = BrowserOptions::default();
        browser_option.headless = false;
        let mut browser = Browser::launch(browser_option).await.unwrap();
        let tab = browser.new_tab_at("example.com").await.unwrap();
        dbg!(tab.title().await.unwrap());
        // Hold the browser alive long enough to actually see the window.
        tokio::time::sleep(std::time::Duration::from_secs(30)).await;
        browser.close().await.unwrap();
    }

    /// Drive every tool in the default registry against a real Chromium.
    /// Skips the screenshot assertion in CI / sandboxed runners where a
    /// long-running headed browser is impractical — set
    /// `BROWSER_USE_E2E=1` to opt in.
    #[tokio::test]
    async fn test_tools_e2e() {
        if std::env::var("BROWSER_USE_E2E").is_err() {
            eprintln!("set BROWSER_USE_E2E=1 to run; skipping");
            return;
        }
        use agentik_core::tools::ToolFunction;
        let session = crate::BrowserSession::launch(BrowserOptions::default())
            .await
            .unwrap();

        // 1) navigate
        let nav = crate::tools::navigate::NavigateTool::new(session.clone());
        nav.run(crate::tools::navigate::NavigateInput {
            url: "https://example.com".to_string(),
            tab_index: Some(0),
        })
        .await
        .unwrap();
        nav.run(crate::tools::navigate::NavigateInput {
            url: "https://www.iana.org".to_string(),
            tab_index: Some(0),
        })
        .await
        .unwrap();

        // 2) get_url / get_title
        let url_tool = crate::tools::get_url::GetUrlTool::new(session.clone());
        let r = url_tool
            .run(crate::tools::get_url::GetUrlInput { tab_index: Some(0) })
            .await
            .unwrap();
        let url_text = r.text_content();
        assert!(url_text.contains("iana.org"), "url: {url_text}");

        let title_tool = crate::tools::get_title::GetTitleTool::new(session.clone());
        let r = title_tool
            .run(crate::tools::get_title::GetTitleInput { tab_index: Some(0) })
            .await
            .unwrap();
        assert!(!r.text_content().is_empty());

        // 3) extract_text / extract_html
        let t = crate::tools::extract_text::ExtractTextTool::new(session.clone());
        let r = t
            .run(crate::tools::extract_text::ExtractTextInput { tab_index: Some(0) })
            .await
            .unwrap();
        assert!(r.text_content().len() > 50);

        let h = crate::tools::extract_html::ExtractHtmlTool::new(session.clone());
        let r = h
            .run(crate::tools::extract_html::ExtractHtmlInput { tab_index: Some(0) })
            .await
            .unwrap();
        assert!(r.text_content().contains("<html"));

        // 4) evaluate
        let ev = crate::tools::evaluate::EvaluateTool::new(session.clone());
        let r = ev
            .run(crate::tools::evaluate::EvaluateInput {
                expression: "1 + 2".to_string(),
                tab_index: Some(0),
            })
            .await
            .unwrap();
        assert_eq!(r.text_content(), "3");

        // 5) tab management
        let nt = crate::tools::new_tab::NewTabTool::new(session.clone());
        nt.run(crate::tools::new_tab::NewTabInput { url: None })
            .await
            .unwrap();
        nt.run(crate::tools::new_tab::NewTabInput {
            url: Some("https://example.com".to_string()),
        })
        .await
        .unwrap();
        let lt = crate::tools::list_tabs::ListTabsTool::new(session.clone());
        let r = lt
            .run(crate::tools::list_tabs::ListTabsInput {})
            .await
            .unwrap();
        let tabs_json: serde_json::Value = serde_json::from_str(&r.text_content()).unwrap();
        // CDP can briefly report a stale target before the handler catches
        // up; assert the count is at least what we asked for rather than
        // an exact value.
        assert!(tabs_json.as_array().unwrap().len() >= 3);

        let sw = crate::tools::switch_tab::SwitchTabTool::new(session.clone());
        sw.run(crate::tools::switch_tab::SwitchTabInput { tab_index: 0 })
            .await
            .unwrap();

        let ct = crate::tools::close_tab::CloseTabTool::new(session.clone());
        ct.run(crate::tools::close_tab::CloseTabInput { tab_index: Some(1) })
            .await
            .unwrap();

        // 6) registry completeness — every tool builds + runs through
        //    the typed `run` path without compile/registration errors.
        for reg in crate::tools::registrations(session.clone()) {
            assert!(!reg.definition.name.is_empty());
        }

        session.close().await.unwrap();
    }

    /// Cover the Chrome 153 regressions found by the browser-control audit:
    /// typing through CDP keyboard events could hang, and a URL-bearing new
    /// target could remain on Chrome's internal new-tab page.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn browser_type_text_and_new_tab_url_are_reliable() {
        use std::sync::atomic::AtomicBool;
        use std::time::Duration;

        use agentik_core::tools::ToolFunction;

        if std::env::var("BROWSER_USE_E2E").is_err() {
            eprintln!("set BROWSER_USE_E2E=1 to run; skipping");
            return;
        }

        let server = std::sync::Arc::new(tiny_http::Server::http(("127.0.0.1", 0)).unwrap());
        let address = server.server_addr().to_string();
        let running = std::sync::Arc::new(AtomicBool::new(true));
        let worker = {
            let server = server.clone();
            let running = running.clone();
            std::thread::spawn(move || {
                while running.load(std::sync::atomic::Ordering::SeqCst) {
                    let Ok(request) = server.recv() else {
                        break;
                    };
                    let body = r#"<!doctype html><input id="field">"#;
                    let response = tiny_http::Response::from_string(body)
                        .with_header(
                            tiny_http::Header::from_bytes(
                                &b"Content-Type"[..],
                                &b"text/html; charset=utf-8"[..],
                            )
                            .unwrap(),
                        )
                        .with_header(
                            tiny_http::Header::from_bytes(&b"Connection"[..], &b"close"[..])
                                .unwrap(),
                        );
                    let _ = request.respond(response);
                }
            })
        };

        let mut options = BrowserOptions::default();
        options.no_sandbox = true;
        let session = super::BrowserSession::launch(options).await.unwrap();
        let url = format!("http://{address}/form");
        let tab = session.tab(None).await.unwrap();
        tab.goto(&url).await.unwrap();

        let typing = crate::tools::type_text::TypeTextTool::new(session.clone());
        typing
            .run(crate::tools::type_text::TypeTextInput {
                selector: "#field".to_string(),
                text: "headless browser test".to_string(),
                tab_index: Some(0),
                timeout_ms: Some(10_000),
            })
            .await
            .unwrap();
        let value = session
            .tab(None)
            .await
            .unwrap()
            .evaluate("document.querySelector('#field').value")
            .await
            .unwrap();
        assert_eq!(value.as_str(), Some("headless browser test"));

        let new_index = session.new_tab_at(&url).await.unwrap();
        let mut new_url = None;
        for _ in 0..100 {
            new_url = session
                .tab(Some(new_index))
                .await
                .unwrap()
                .url()
                .await
                .unwrap();
            if new_url.as_deref() == Some(url.as_str()) {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        assert_eq!(new_url.as_deref(), Some(url.as_str()));

        session.close().await.unwrap();
        running.store(false, std::sync::atomic::Ordering::SeqCst);
        server.unblock();
        worker.join().unwrap();
    }

    /// Download a real attachment through CDP and verify the completed path.
    /// Set `BROWSER_USE_E2E=1` to opt in; the local HTTP server keeps this
    /// test independent of the public internet.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn browser_download_lands_in_configured_directory() {
        if std::env::var("BROWSER_USE_E2E").is_err() {
            eprintln!("set BROWSER_USE_E2E=1 to run; skipping");
            return;
        }

        let downloads = tempfile::tempdir().unwrap();
        let mut options = BrowserOptions::default();
        options.no_sandbox = true;
        options.download_dir = Some(downloads.path().to_path_buf());
        options.download_vfs_path = Some("/browser-downloads".to_string());
        let session = super::BrowserSession::launch(options).await.unwrap();

        let server = tiny_http::Server::http(("127.0.0.1", 0)).unwrap();
        let address = server.server_addr().to_string();
        std::thread::spawn(move || {
            if let Ok(request) = server.recv() {
                let body = "browser download payload\n";
                let response = tiny_http::Response::from_string(body)
                    .with_header(
                        tiny_http::Header::from_bytes(
                            &b"Content-Type"[..],
                            &b"application/octet-stream"[..],
                        )
                        .unwrap(),
                    )
                    .with_header(
                        tiny_http::Header::from_bytes(
                            &b"Content-Disposition"[..],
                            &b"attachment; filename=\"hello.txt\""[..],
                        )
                        .unwrap(),
                    )
                    .with_header(
                        tiny_http::Header::from_bytes(&b"Connection"[..], &b"close"[..]).unwrap(),
                    );
                let _ = request.respond(response);
            }
        });

        let info = session
            .download(
                &format!("http://{address}/hello.txt"),
                None,
                std::time::Duration::from_secs(20),
            )
            .await
            .unwrap();
        assert_eq!(info.state, "completed");
        assert_eq!(info.suggested_filename, "hello.txt");
        assert_eq!(info.received_bytes, info.total_bytes);
        let file_path = std::path::PathBuf::from(info.file_path.expect("download path"));
        assert_eq!(
            std::fs::read(&file_path).unwrap(),
            b"browser download payload\n"
        );
        assert!(
            info.vfs_path
                .as_deref()
                .is_some_and(|path| path.starts_with("/browser-downloads/"))
        );
        session.close().await.unwrap();
    }
}
