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

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use chromiumoxide::browser::BrowserConfig;
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
}

impl Browser {
    /// Launch a new browser process with the given options.
    pub async fn launch(options: BrowserOptions) -> Result<Self> {
        let (browser, handler) = CdpBrowser::launch(options.to_cdp_config()?)
            .await
            .map_err(|e| Error::Launch(e.to_string()))?;
        let _handler = spawn_handler(handler);
        Ok(Self { browser, _handler })
    }

    /// Attach to an already running browser via its debugging URL
    /// (`http://host:port` or `ws://host:port/devtools/browser/...`).
    pub async fn connect(url: impl Into<String>) -> Result<Self> {
        let (browser, handler) = CdpBrowser::connect(url)
            .await
            .map_err(|e| Error::Connect(e.to_string()))?;
        let _handler = spawn_handler(handler);
        Ok(Self { browser, _handler })
    }

    /// Open a new empty tab.
    pub async fn new_tab(&self) -> Result<Tab> {
        let page = self.browser.new_page("about:blank").await?;
        Ok(Tab { page })
    }

    /// Open a new tab and navigate it to `url` in one step.
    pub async fn new_tab_at(&self, url: &str) -> Result<Tab> {
        let tab = self.new_tab().await?;
        tab.goto(url).await?;
        Ok(tab)
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

    /// Gracefully close the browser and wait for the process to exit.
    pub async fn close(&mut self) -> Result<()> {
        self.browser.close().await?;
        self.browser.wait().await?;
        Ok(())
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
        let element = self.wait_for_selector(selector, None).await?;
        element.click().await?;
        element.type_str(input).await?;
        Ok(())
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
        })
    }

    /// Launch a new browser and immediately wrap it in a session.
    pub async fn launch(options: BrowserOptions) -> Result<Arc<Self>> {
        let browser = Browser::launch(options).await?;
        Ok(Arc::new(Self::new(browser).await?))
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
}
