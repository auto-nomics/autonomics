//! [`BibHttpOptions`] — configuration for the `reqwest::Client` (and
//! therefore every literature source / Europe PMC call) shared across
//! the whole process via [`crate::BibShared`].
//!
//! Every field is `None` by default and falls back to a sensible default
//! when `BibShared::open` builds the underlying `reqwest::Client`. This
//! keeps the type `Clone + Serialize + Deserialize + Debug` so it can
//! ride inside a `RuntimeConfig` and round-trip through TOML / JSON.

use std::time::Duration;

/// HTTP client configuration shared by every literature source.
///
/// All fields are optional; unset fields fall through to the defaults
/// baked into [`BibHttpOptions::apply_to`].
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct BibHttpOptions {
    /// Value for the `User-Agent` header. Default:
    /// `"autonomics-bib-base"`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user_agent: Option<String>,

    /// Connect timeout (time to establish the TCP/TLS connection).
    /// Default: 10 s.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "duration_secs_opt"
    )]
    pub connect_timeout: Option<Duration>,

    /// Per-request total timeout (read timeout). Default: 60 s.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "duration_secs_opt"
    )]
    pub request_timeout: Option<Duration>,

    /// HTTP/HTTPS proxy URL (e.g. `http://proxy.corp:3128`,
    /// `socks5://10.0.0.1:1080`). Default: none (use direct connection).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub proxy_url: Option<String>,

    /// When `true`, skip TLS certificate verification. Intended for
    /// corp-network MITM proxies during development only.
    /// Default: `false`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub accept_invalid_certs: Option<bool>,
}

impl BibHttpOptions {
    /// Apply these options onto a [`reqwest::ClientBuilder`]. The
    /// builder is consumed; the caller finishes with `.build()`.
    ///
    /// Unset fields fall back to conservative defaults (10 s connect /
    /// 60 s request timeouts, no proxy, valid TLS).
    pub fn apply_to(self, builder: reqwest::ClientBuilder) -> reqwest::ClientBuilder {
        let user_agent = self
            .user_agent
            .unwrap_or_else(|| "autonomics-bib-base".to_string());
        let connect_timeout = self.connect_timeout.unwrap_or(Duration::from_secs(10));
        let request_timeout = self.request_timeout.unwrap_or(Duration::from_secs(60));
        let mut builder = builder
            .user_agent(user_agent)
            .connect_timeout(connect_timeout)
            .timeout(request_timeout);

        if let Some(url) = self.proxy_url.as_deref() {
            // `reqwest::Proxy::all` already accepts both http:// and
            // socks5:// schemes.
            if let Ok(proxy) = reqwest::Proxy::all(url) {
                builder = builder.proxy(proxy);
            }
            // If the URL is malformed, silently fall back to direct
            // connection. Better to log a warning in the future, but
            // avoid surfacing config errors at runtime hot paths.
        }

        if self.accept_invalid_certs.unwrap_or(false) {
            builder = builder.danger_accept_invalid_certs(true);
        }

        builder
    }

    /// Build the final `reqwest::Client`. If building fails (extremely
    /// rare — only on system resource exhaustion), fall back to a
    /// minimal default client so the process can keep running.
    pub fn build_client(self) -> reqwest::Client {
        self.apply_to(reqwest::Client::builder())
            .build()
            .unwrap_or_else(|_| reqwest::Client::new())
    }
}

mod duration_secs_opt {
    //! `serde` adapter for `Option<Duration>` represented as whole seconds.
    use std::time::Duration;

    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S: Serializer>(
        value: &Option<Duration>,
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        match value {
            Some(d) => serializer.serialize_some(&d.as_secs()),
            None => serializer.serialize_none(),
        }
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(
        deserializer: D,
    ) -> Result<Option<Duration>, D::Error> {
        let secs = Option::<u64>::deserialize(deserializer)?;
        Ok(secs.map(Duration::from_secs))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn apply_to_uses_defaults_when_empty() {
        let builder = BibHttpOptions::default().apply_to(reqwest::Client::builder());
        // We can't introspect the builder, but we can build it and
        // confirm the resulting client is usable.
        let _client = builder.build().expect("build");
    }

    #[test]
    fn apply_to_overrides_user_agent() {
        let opts = BibHttpOptions {
            user_agent: Some("my-agent/1.0".into()),
            ..Default::default()
        };
        let client = opts.build_client();
        // Cannot directly read user_agent from reqwest::Client, but we
        // can verify the type doesn't panic and produces a valid
        // client.
        let _ = client.get("http://example.invalid");
    }

    #[test]
    fn roundtrip_through_json() {
        let opts = BibHttpOptions {
            user_agent: Some("a".into()),
            connect_timeout: Some(Duration::from_secs(5)),
            request_timeout: Some(Duration::from_secs(30)),
            proxy_url: Some("socks5://localhost:1080".into()),
            accept_invalid_certs: Some(true),
        };
        let json = serde_json::to_string(&opts).unwrap();
        let back: BibHttpOptions = serde_json::from_str(&json).unwrap();
        assert_eq!(back, opts);
    }

    #[test]
    fn defaults_serialize_as_empty_object() {
        let json = serde_json::to_string(&BibHttpOptions::default()).unwrap();
        assert_eq!(json, "{}");
    }
}
