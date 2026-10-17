use thiserror::Error;

pub type Result<T> = std::result::Result<T, Error>;

/// Errors produced while launching or driving a browser.
#[derive(Debug, Error)]
pub enum Error {
    /// The browser process could not be launched.
    #[error("failed to launch browser: {0}")]
    Launch(String),

    /// A running browser could not be connected to.
    #[error("failed to connect to browser: {0}")]
    Connect(String),

    /// An error reported by the Chrome DevTools Protocol.
    #[error("cdp error: {0}")]
    Cdp(#[from] chromiumoxide::error::CdpError),

    /// An operation exceeded its deadline.
    #[error("`{operation}` timed out after {timeout_ms}ms")]
    Timeout {
        operation: &'static str,
        timeout_ms: u64,
    },

    /// A selector matched nothing on the page.
    #[error("no element matches selector `{0}`")]
    ElementNotFound(String),

    /// A URL was not usable for navigation.
    #[error("invalid url `{0}`")]
    InvalidUrl(String),

    /// A newly created tab stayed on an internal page after navigation.
    #[error("new tab failed to navigate away from its initial page")]
    NewTabNavigation,

    /// Downloads were requested without a configured download directory.
    #[error("browser downloads are disabled; no download directory is configured")]
    DownloadsDisabled,

    /// A local file operation failed (e.g. writing a screenshot).
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),

    /// A JSON (de)serialization failed while unpacking an evaluation result.
    #[error("json error: {0}")]
    Json(#[from] serde_json::Error),

    /// A `tab_index` argument referred to a slot past the end of the tab list.
    #[error("tab index {index} out of range ({total} tabs open)")]
    TabIndexOutOfRange { index: usize, total: usize },
}
