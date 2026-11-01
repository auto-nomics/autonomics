//! Configuration for [`RuntimeHost`](crate::RuntimeHost).
//!
//! Every hard-coded path, env-var override, and prompt string that was
//! previously scattered across `runtime.rs` / `tools.rs` / `app.rs` is
//! collected here so that multiple independent agent runtimes — each with
//! its own storage root, history DB, bibliography, prompts, etc. — can be
//! instantiated side-by-side in a future multi-agent system.
//!
//! ## Module layout (one configuration unit per submodule)
//!
//! - [`defaults`] — hard-coded default paths/filenames and env-var names.
//! - [`rsi`] — plugin RSI + environment-development configuration.
//! - [`runtime`] — the resolved [`RuntimeConfig`] and env/default resolution.
//! - [`builder`] — the [`RuntimeConfigBuilder`] overlay.
//! - [`prompt`] — system-prompt sections and [`build_system_prompt`].
//!
//! ## Construction
//!
//! ```ignore
//! use runtime::config::RuntimeConfig;
//!
//! // Defaults — same behaviour as before.
//! let cfg = RuntimeConfig::default();
//!
//! // Customised for a specific agent.
//! let cfg = RuntimeConfig::builder()
//!     .name("literature-agent")
//!     .data_dir("/data/agents/lit")
//!     .opengwas_token_env("OPENGWAS_TOKEN_LIT")
//!     .build();
//! ```
//!
//! ## Precedence
//!
//! For every field the resolution order is:
//! 1. Explicitly set on the builder (`Builder::build()` copies it verbatim).
//! 2. Environment variable named on the field's doc comment.
//! 3. Hard-coded default.
//!
//! `RuntimeConfig::resolve()` performs step 2→3; the builder performs
//! step 1 by letting the caller override the *entire* resolved config.

mod builder;
mod defaults;
mod prompt;
mod rsi;
mod runtime;

pub use builder::RuntimeConfigBuilder;
pub use defaults::{
    ENV_APP_DB, ENV_BIB_DB, ENV_BROWSER_DOWNLOAD_DIR, ENV_DATA_DIR, ENV_GENERATE_MEMORY,
    ENV_HTTP_CONNECT_TIMEOUT_SECS, ENV_HTTP_PROXY, ENV_HTTP_REQUEST_TIMEOUT_SECS,
    ENV_HTTP_USER_AGENT, ENV_KMS_DB, ENV_KNOWLEDGE_DB, ENV_OPENGWAS_CACHE_DIR, ENV_OPENGWAS_TOKEN,
    ENV_STATE_DIR, ENV_USE_MEMORY, ENV_WIKI_DB, ENV_WRITING_DB,
};
pub use prompt::{PromptCapabilities, build_system_prompt, default_system_prompt};
pub use rsi::{EnvironmentDevConfig, PluginRsiConfig};
pub use runtime::RuntimeConfig;

#[cfg(test)]
mod tests;
