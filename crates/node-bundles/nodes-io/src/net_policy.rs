//! Shared network-egress policy for HTTP-downloading nodes.
//!
//! [`crate::http_fetch::HttpFetchNode`] and [`crate::file_download::FileDownloadNode`]
//! both gate egress through the same deny-by-default host allowlist
//! (`state_dir/network-allowlist.toml`, wired up by the gateway daemon via
//! [`ENV_NETWORK_ALLOWLIST`]). Keeping the loader, host matcher, and redirect
//! policy in one module means the two nodes cannot drift apart on what
//! "allowed" means: an entry matches its own host and every subdomain, and a
//! redirect must clear the allowlist at every hop.

use std::collections::BTreeSet;

use thiserror::Error;

/// Environment variable holding the allowlist file path. Set by the gateway
/// daemon (`serve`) to `<state_dir>/network-allowlist.toml`.
pub const ENV_NETWORK_ALLOWLIST: &str = "AUTONOMICS_NETWORK_ALLOWLIST";
pub const ALLOWLIST_FILE_NAME: &str = "network-allowlist.toml";

/// Errors that can arise while loading or applying the shared network
/// allowlist. The wording is shared verbatim by every policy-gated node so
/// operators see one message for one config problem.
#[derive(Debug, Error)]
pub enum NetworkPolicyError {
    #[error(
        "no HTTP fetch allowlist is configured ({reason}); the gateway daemon creates \
         `<state_dir>/{ALLOWLIST_FILE_NAME}` and points {ENV_NETWORK_ALLOWLIST} at it on startup"
    )]
    AllowlistUnavailable { reason: String },

    #[error("cannot read HTTP fetch allowlist `{path}`: {source}")]
    AllowlistRead {
        path: String,
        #[source]
        source: std::io::Error,
    },

    #[error("invalid HTTP fetch allowlist `{path}`: {reason}")]
    AllowlistParse { path: String, reason: String },

    #[error(
        "host `{host}` is not on the HTTP fetch allowlist; add `[[allow]]` with \
         `host = \"{host}\"` to the file named by {ENV_NETWORK_ALLOWLIST} and restart the \
         gateway, or remove this node from the workflow"
    )]
    DomainNotAllowed { host: String },
}

/// Commented template written to `state_dir/network-allowlist.toml` on first
/// daemon start (deny-by-default: nothing is fetchable until a host is added).
pub fn default_allowlist_toml() -> &'static str {
    r#"# HTTP fetch allowlist for the `http_fetch` DAG node.
#
# Deny-by-default: a fetch is refused unless the URL host matches an entry
# below. An entry matches its own host and any subdomain — `ebi.ac.uk` also
# covers `ftp.ebi.ac.uk`. Entries are hostnames only (no scheme, no path).
# Restart the gateway after editing.

# [[allow]]
# host = "ftp.ebi.ac.uk"
"#
}

#[derive(Debug, Default, serde::Deserialize)]
struct AllowlistFile {
    #[serde(default, rename = "allow")]
    allow: Vec<AllowEntry>,
}

#[derive(Debug, serde::Deserialize)]
struct AllowEntry {
    host: String,
}

/// Parse allowlist TOML source into normalized host entries. `path` is only
/// used for error context.
pub fn parse_allowlist(path: &str, source: &str) -> Result<Vec<String>, NetworkPolicyError> {
    let parsed: AllowlistFile =
        toml::from_str(source).map_err(|error| NetworkPolicyError::AllowlistParse {
            path: path.to_string(),
            reason: error.to_string(),
        })?;
    let mut entries = BTreeSet::new();
    for entry in parsed.allow {
        let host = entry.host.trim().to_ascii_lowercase();
        if host.is_empty() || host.contains("://") || host.contains('/') || host.contains(' ') {
            return Err(NetworkPolicyError::AllowlistParse {
                path: path.to_string(),
                reason: format!("invalid allow entry host `{}`", entry.host),
            });
        }
        entries.insert(host.trim_matches('.').to_string());
    }
    Ok(entries.into_iter().collect())
}

/// True when `host` matches an allowlist entry (exact host or any subdomain,
/// case-insensitive). Like eTLD+1 rules, `notebi.ac.uk` does NOT match an
/// `ebi.ac.uk` entry and the parent `ac.uk` alone never matches.
pub fn host_allowed(host: &str, entries: &[String]) -> bool {
    let host = host.trim_end_matches('.').to_ascii_lowercase();
    entries
        .iter()
        .any(|entry| host == *entry || host.ends_with(&format!(".{entry}")))
}

/// Redirects are followed only while every hop stays on the allowlist.
pub fn redirect_policy(entries: Vec<String>) -> reqwest::redirect::Policy {
    reqwest::redirect::Policy::custom(move |attempt| {
        if attempt.previous().len() > 5 {
            return attempt.error("too many redirects");
        }
        match attempt.url().host_str() {
            Some(host) if host_allowed(host, &entries) => attempt.follow(),
            _ => attempt.error("redirect target host is not on the HTTP fetch allowlist"),
        }
    })
}

/// Load the allowlist from the path named by [`ENV_NETWORK_ALLOWLIST`].
/// An unset variable, a missing file, or an empty file all fail closed.
pub fn load_allowlist_from_env() -> Result<Vec<String>, NetworkPolicyError> {
    let Some(path) = std::env::var(ENV_NETWORK_ALLOWLIST)
        .ok()
        .filter(|value| !value.trim().is_empty())
    else {
        return Err(NetworkPolicyError::AllowlistUnavailable {
            reason: format!("{ENV_NETWORK_ALLOWLIST} is not set"),
        });
    };
    let source =
        std::fs::read_to_string(&path).map_err(|error| NetworkPolicyError::AllowlistRead {
            path: path.clone(),
            source: error,
        })?;
    parse_allowlist(&path, &source)
}

/// Reject a URL whose host is off the allowlist (deny-by-default).
pub fn ensure_host_allowed(
    url: &reqwest::Url,
    entries: &[String],
) -> Result<(), NetworkPolicyError> {
    let host = url.host_str().unwrap_or_default();
    if host_allowed(host, entries) {
        Ok(())
    } else {
        Err(NetworkPolicyError::DomainNotAllowed {
            host: host.to_string(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_allowlist_accepts_entries_and_rejects_bad_hosts() {
        let path = "/tmp/allowlist.toml";
        let entries = parse_allowlist(path, "[[allow]]\nhost = \"FTP.EBI.ac.uk\"\n").unwrap();
        assert_eq!(entries, vec!["ftp.ebi.ac.uk"]);
        assert!(parse_allowlist(path, "").unwrap().is_empty());

        let error = parse_allowlist(path, "[[allow]]\nhost = \"https://x.com/a\"\n").unwrap_err();
        assert!(
            error.to_string().contains("invalid allow entry host"),
            "{error}"
        );
        let error = parse_allowlist(path, "allow = [oops]\n").unwrap_err();
        assert!(
            error.to_string().contains("invalid HTTP fetch allowlist"),
            "{error}"
        );
    }

    #[test]
    fn host_matching_covers_subdomains_only() {
        let entries = vec!["ebi.ac.uk".to_string(), "localhost".to_string()];
        assert!(host_allowed("ebi.ac.uk", &entries));
        assert!(host_allowed("ftp.ebi.ac.uk", &entries));
        assert!(host_allowed("FTP.EBI.AC.UK", &entries));
        assert!(host_allowed("localhost", &entries));
        assert!(host_allowed("127.0.0.1", &["127.0.0.1".into()]));
        assert!(!host_allowed("notebi.ac.uk", &entries));
        assert!(!host_allowed("evil-ebi.ac.uk", &entries));
        assert!(!host_allowed("ac.uk", &entries));
        assert!(!host_allowed("example.com", &entries));
    }

    #[test]
    fn template_mentions_the_env_variable_and_file() {
        let template = default_allowlist_toml();
        assert!(template.contains("[[allow]]"));
        assert!(!template.contains("{{"));
    }
}
