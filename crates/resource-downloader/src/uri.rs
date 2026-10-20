use std::net::IpAddr;

use serde::{Deserialize, Serialize};
use url::Url;

use crate::error::{DownloadError, Result};

const MAX_REDIRECTS: usize = 5;

/// A validated external resource address.
///
/// Userinfo is intentionally discarded while parsing. Credentials belong in
/// host-managed profiles, never in model-visible tool input or logs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResourceUri {
    #[serde(rename = "scheme")]
    pub scheme: String,
    #[serde(rename = "host")]
    pub host: String,
    #[serde(rename = "port")]
    pub port: Option<u16>,
    #[serde(rename = "path")]
    pub path: String,
    #[serde(rename = "query")]
    pub query: Option<String>,
}

impl ResourceUri {
    pub fn parse(input: &str) -> Result<Self> {
        Self::parse_with_options(input, false)
    }

    /// Test/internal helper for controlled local servers. Production callers
    /// must use [`Self::parse`], which always rejects direct loopback IPs.
    #[doc(hidden)]
    pub fn parse_for_tests(input: &str) -> Result<Self> {
        Self::parse_with_options(input, true)
    }

    pub(crate) fn parse_with_options(input: &str, allow_direct_ip_for_tests: bool) -> Result<Self> {
        let raw = input.trim();
        if raw.is_empty() {
            return Err(DownloadError::InvalidSource {
                resource: input.to_string(),
                message: "must not be empty".to_string(),
            });
        }

        let url = Url::parse(raw).map_err(|error| DownloadError::InvalidSource {
            resource: input.to_string(),
            message: error.to_string(),
        })?;

        let scheme = url.scheme().to_ascii_lowercase();
        if scheme != "https" && scheme != "http" {
            return Err(DownloadError::InvalidSource {
                resource: input.to_string(),
                message: format!("unsupported scheme `{scheme}`; only http/https is enabled"),
            });
        }

        let host = url
            .host_str()
            .ok_or_else(|| DownloadError::InvalidSource {
                resource: input.to_string(),
                message: "host is required".to_string(),
            })?
            .trim_end_matches('.')
            .trim_start_matches('[')
            .trim_end_matches(']')
            .to_ascii_lowercase();

        if let Ok(ip) = host.parse::<IpAddr>() {
            if !allow_direct_ip_for_tests && is_forbidden_ip(ip) {
                return Err(DownloadError::Denied {
                    resource: input.to_string(),
                    message: format!("direct IP address {ip} is not allowed"),
                });
            }
        }

        let Some(path) = percent_encoding::percent_decode(url.path().as_bytes())
            .decode_utf8()
            .ok()
            .map(|value| value.to_string())
        else {
            return Err(DownloadError::InvalidSource {
                resource: input.to_string(),
                message: "path is not valid UTF-8".to_string(),
            });
        };

        Ok(Self {
            scheme,
            host,
            port: url.port(),
            path,
            query: url.query().map(str::to_string),
        })
    }

    pub fn origin_form(&self) -> String {
        match &self.query {
            Some(query) => format!("{}?{query}", self.path),
            None => self.path.clone(),
        }
    }

    /// Host plus default-scheme port, used as a stable scheduler/policy key.
    pub fn authority_key(&self) -> String {
        match self.port {
            Some(port) => format!("{}:{port}", self.host),
            None => self.host.clone(),
        }
    }

    /// A safe display form without userinfo and query credentials.
    pub fn display(&self) -> String {
        let mut safe = format!("{}://{}", self.scheme, self.host);
        if let Some(port) = self.port {
            safe.push_str(&format!(":{port}"));
        }
        safe.push_str(&self.path);
        if let Some(query) = &self.query {
            safe.push('?');
            safe.push_str(query);
        }
        safe
    }
}

pub fn is_forbidden_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => {
            v4.is_loopback()
                || v4.is_private()
                || v4.is_link_local()
                || v4.is_broadcast()
                || v4.is_unspecified()
                || v4.is_documentation()
        }
        IpAddr::V6(v6) => {
            v6.is_loopback()
                || v6.is_unspecified()
                || v6.is_multicast()
                || matches!(v6.segments(), [0, 0, 0, 0, 0, 0, 0, 1])
                || matches!(v6.segments(), [0xfe80, ..])
                || matches!(v6.segments(), [0xfc00, ..])
                || matches!(v6.segments(), [0xfd00, ..])
        }
    }
}

pub fn redirect_is_allowed(source: &str, redirects: &[Url]) -> Result<()> {
    if redirects.len() > MAX_REDIRECTS {
        return Err(DownloadError::request(
            source,
            format!("too many redirects (more than {MAX_REDIRECTS})"),
        ));
    }
    for url in redirects {
        let _checked = ResourceUri::parse(url.as_str())?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_https_uri_and_drops_userinfo() {
        let uri = ResourceUri::parse("https://EXAMPLE.com:8443/a%20b?x=1").unwrap();
        assert_eq!(uri.host, "example.com");
        assert_eq!(uri.port, Some(8443));
        assert_eq!(uri.path, "/a b");
        assert_eq!(uri.origin_form(), "/a b?x=1");
    }

    #[test]
    fn rejects_non_https_private_and_loopback_sources() {
        assert!(ResourceUri::parse("ftp://example.com/x").is_err());
        assert!(ResourceUri::parse("https://127.0.0.1/x").is_err());
        assert!(ResourceUri::parse("https://192.168.1.1/x").is_err());
        assert!(ResourceUri::parse("https://[::1]/x").is_err());
    }
}
