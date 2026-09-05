use std::time::Duration;

use reqwest::Client;
use tracing::debug;

use crate::error::{EasyScholarError, Result};
use crate::types::*;

/// Default EasyScholar open-API endpoint.
const BASE_URL: &str = "https://www.easyscholar.cc/open/getPublicationRank";

/// Request timeout — EasyScholar is slow at times; 10s mirrors jayread.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);

fn env_endpoint() -> String {
    std::env::var("ENDPOINT_EASYSCHOLAR_URL").unwrap_or_else(|_| BASE_URL.to_string())
}

/// An async client for the [EasyScholar publication-rank API](
/// https://www.easyscholar.cc), which resolves journal-level metrics
/// (impact factor, JCR/SSCI quartiles, CAS 分区) **by journal name**.
///
/// The API requires a user-registered `secretKey`. Without one, every
/// method degrades to "no data" (`Ok(None)` / invalid) rather than failing
/// — enrichment pipelines can call it unconditionally.
///
/// The key is held behind a `RwLock` so a settings UI can hot-swap it at
/// runtime (`[`set_key`](Self::set_key)`) without rebuilding the client.
///
/// # Example
///
/// ```no_run
/// # async fn run() -> easyscholar::error::Result<()> {
/// let client = easyscholar::EasyscholarClient::from_env();
/// if let Some(rank) = client.fetch_journal_rank("Nature Medicine").await? {
///     println!("IF {:?} / JCR {:?}", rank.impact_factor, rank.jcr_quartile);
/// }
/// # Ok(())
/// # }
/// ```
#[derive(Debug)]
pub struct EasyscholarClient {
    http: Client,
    key: std::sync::RwLock<Option<String>>,
}

impl Default for EasyscholarClient {
    fn default() -> Self {
        Self::new(None)
    }
}

impl EasyscholarClient {
    /// Create a client with an explicit API key (`None` = unconfigured).
    pub fn new(api_key: Option<String>) -> Self {
        Self {
            http: Client::builder()
                .timeout(REQUEST_TIMEOUT)
                .user_agent("autonomics-easyscholar/0.1")
                .build()
                .expect("reqwest client builder"),
            key: std::sync::RwLock::new(api_key.filter(|k| !k.trim().is_empty())),
        }
    }

    /// Create a client reading the key from `EASYSCHOLAR_KEY`.
    pub fn from_env() -> Self {
        Self::new(std::env::var("EASYSCHOLAR_KEY").ok())
    }

    /// Hot-swap the API key at runtime (settings UI writes through here).
    pub fn set_key(&self, api_key: Option<String>) {
        let mut guard = self.key.write().expect("easyscholar key lock");
        *guard = api_key.filter(|k| !k.trim().is_empty());
    }

    /// Current API key, if configured.
    pub fn api_key(&self) -> Option<String> {
        self.key.read().expect("easyscholar key lock").clone()
    }

    /// Fetch journal metrics by publication name.
    ///
    /// Returns `Ok(None)` for every "no answer" condition — unknown journal,
    /// unconfigured key, non-2xx HTTP, business code ≠ 200, or an all-empty
    /// rank block. Transport-level failures surface as `Err`.
    pub async fn fetch_journal_rank(&self, publication_name: &str) -> Result<Option<JournalRank>> {
        let name = publication_name.trim();
        if name.is_empty() {
            return Ok(None);
        }
        let Some(api_key) = self.api_key() else {
            debug!("easyscholar key not configured, skipping journal rank fetch");
            return Ok(None);
        };

        let resp = self
            .http
            .get(env_endpoint())
            .query(&[("secretKey", api_key.as_str()), ("publicationName", name)])
            .send()
            .await?;
        let status = resp.status().as_u16();
        let body = resp.text().await?;
        if !(200..300).contains(&status) {
            debug!(status, journal = name, "easyscholar non-2xx, treating as no data");
            return Ok(None);
        }

        let payload: EasyScholarResponse = serde_json::from_str(&body)?;
        if payload.code != 200 {
            debug!(code = payload.code, journal = name, "easyscholar business error, treating as no data");
            return Ok(None);
        }

        let rank_data = payload
            .data
            .and_then(|d| d.official_rank)
            .and_then(|r| r.all);
        let Some(raw) = rank_data else {
            return Ok(None);
        };

        let rank = JournalRank {
            impact_factor: parse_factor(&raw.sciif),
            impact_factor_5: parse_factor(&raw.sciif5),
            jcr_quartile: clean_rank_string(raw.sci),
            ssci_quartile: clean_rank_string(raw.ssci),
            cas_quartile_base: clean_rank_string(raw.sci_base),
            cas_quartile: clean_rank_string(raw.sci_up),
            cas_small: clean_rank_string(raw.sci_up_small),
            cas_top: parse_top_flag(raw.sci_up_top.as_deref()),
            cas_warning: clean_rank_string(raw.sciwarn),
        };

        if rank.has_any() {
            Ok(Some(rank))
        } else {
            Ok(None)
        }
    }

    /// Validate a candidate API key with a minimal probe request
    /// (`publicationName=Nature`).
    ///
    /// Returns `(valid, message)`; the message is user-facing (empty on
    /// success) so a settings dialog can surface it directly.
    pub async fn validate(&self, api_key: &str) -> (bool, String) {
        if api_key.trim().is_empty() {
            return (false, "EASYSCHOLAR_KEY not configured".to_string());
        }

        let resp = match self
            .http
            .get(env_endpoint())
            .query(&[("secretKey", api_key.trim()), ("publicationName", "Nature")])
            .send()
            .await
        {
            Ok(resp) => resp,
            Err(e) if e.is_timeout() => {
                return (false, "EasyScholar connection timeout".to_string())
            }
            Err(e) => {
                return (false, format!("EasyScholar connection error: {e}"));
            }
        };
        if !resp.status().is_success() {
            return (false, format!("EasyScholar HTTP {}", resp.status()));
        }

        match resp.json::<EasyScholarResponse>().await {
            Ok(data) => match data.code {
                200 => (true, String::new()),
                40002 => (false, "EasyScholar API key invalid".to_string()),
                code => (
                    false,
                    format!(
                        "EasyScholar error: {}",
                        data.msg.unwrap_or_else(|| code.to_string())
                    ),
                ),
            },
            Err(e) => (false, format!("Failed to parse EasyScholar response: {e}")),
        }
    }

    /// Convenience for validating the currently configured key.
    pub async fn validate_current(&self) -> (bool, String) {
        match self.api_key() {
            Some(key) => self.validate(&key).await,
            None => (false, "EASYSCHOLAR_KEY not configured".to_string()),
        }
    }
}

/// Clean a rank string: trim whitespace and trailing Chinese/English full
/// stops; map empty results to `None`.
fn clean_rank_string(value: Option<String>) -> Option<String> {
    value
        .map(|v| {
            v.trim()
                .trim_end_matches('。')
                .trim_end_matches('.')
                .to_string()
        })
        .filter(|v| !v.is_empty())
}

/// Parse an impact-factor string into `f64`; unparseable values become
/// `None` (never fail the whole fetch because one field is dirty).
fn parse_factor(value: &Option<String>) -> Option<f64> {
    clean_rank_string(value.clone())?
        .parse::<f64>()
        .ok()
        .filter(|f| f.is_finite())
}

/// Parse the CAS top-journal flag. EasyScholar sends free-form text; only
/// explicit falsy spellings count as `Some(false)`, any other non-empty
/// value counts as `Some(true)`.
fn parse_top_flag(value: Option<&str>) -> Option<bool> {
    let cleaned = clean_rank_string(value.map(|v| v.to_string()))?;
    Some(!matches!(cleaned.as_str(), "0" | "false" | "FALSE" | "False"))
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"{
        "code": 200,
        "msg": null,
        "data": {
            "officialRank": {
                "all": {
                    "sciif": "82.9。",
                    "sciif5": "91.3",
                    "sci": "Q1",
                    "ssci": "",
                    "sciBase": "1区",
                    "sciUp": "1区",
                    "sciUpSmall": "医学1区",
                    "sciUpTop": "顶刊",
                    "sciwarn": ""
                }
            }
        }
    }"#;

    #[test]
    fn parses_and_normalizes_wire_fields() {
        let resp: EasyScholarResponse = serde_json::from_str(SAMPLE).unwrap();
        assert_eq!(resp.code, 200);
        let raw = resp.data.unwrap().official_rank.unwrap().all.unwrap();

        let rank = JournalRank {
            impact_factor: parse_factor(&raw.sciif),
            impact_factor_5: parse_factor(&raw.sciif5),
            jcr_quartile: clean_rank_string(raw.sci),
            ssci_quartile: clean_rank_string(raw.ssci),
            cas_quartile_base: clean_rank_string(raw.sci_base),
            cas_quartile: clean_rank_string(raw.sci_up),
            cas_small: clean_rank_string(raw.sci_up_small),
            cas_top: parse_top_flag(raw.sci_up_top.as_deref()),
            cas_warning: clean_rank_string(raw.sciwarn),
        };

        assert_eq!(rank.impact_factor, Some(82.9));
        assert_eq!(rank.impact_factor_5, Some(91.3));
        assert_eq!(rank.jcr_quartile.as_deref(), Some("Q1"));
        assert_eq!(rank.ssci_quartile, None);
        assert_eq!(rank.cas_quartile.as_deref(), Some("1区"));
        assert_eq!(rank.cas_top, Some(true));
        assert_eq!(rank.cas_warning, None);
        assert!(rank.has_any());
    }

    #[test]
    fn clean_rank_string_strips_trailing_stops() {
        assert_eq!(
            clean_rank_string(Some(" Q2.".to_string())).as_deref(),
            Some("Q2")
        );
        assert_eq!(
            clean_rank_string(Some("Q2。".to_string())).as_deref(),
            Some("Q2")
        );
        assert_eq!(clean_rank_string(Some("   ".to_string())), None);
        assert_eq!(clean_rank_string(None), None);
    }

    #[test]
    fn parse_factor_rejects_dirty_values() {
        assert_eq!(parse_factor(&Some("12.3".into())), Some(12.3));
        assert_eq!(parse_factor(&Some("Q1".into())), None);
        assert_eq!(parse_factor(&Some("".into())), None);
        assert_eq!(parse_factor(&None), None);
    }

    #[test]
    fn parse_top_flag_distinguishes_falsy_spellings() {
        assert_eq!(parse_top_flag(Some("顶刊")), Some(true));
        assert_eq!(parse_top_flag(Some("0")), Some(false));
        assert_eq!(parse_top_flag(Some("false")), Some(false));
        assert_eq!(parse_top_flag(Some("")), None);
        assert_eq!(parse_top_flag(None), None);
    }

    #[test]
    fn all_empty_rank_block_reports_no_data() {
        let raw = RankData {
            sciif: None,
            sciif5: Some("".into()),
            sci: None,
            ssci: None,
            sci_base: None,
            sci_up: None,
            sci_up_small: None,
            sci_up_top: None,
            sciwarn: None,
        };
        // round-trip through the same normalization fetch_journal_rank uses
        let rank = JournalRank {
            impact_factor: parse_factor(&raw.sciif),
            impact_factor_5: parse_factor(&raw.sciif5),
            jcr_quartile: clean_rank_string(raw.sci),
            ssci_quartile: clean_rank_string(raw.ssci),
            cas_quartile_base: clean_rank_string(raw.sci_base),
            cas_quartile: clean_rank_string(raw.sci_up),
            cas_small: clean_rank_string(raw.sci_up_small),
            cas_top: parse_top_flag(raw.sci_up_top.as_deref()),
            cas_warning: clean_rank_string(raw.sciwarn),
        };
        assert!(!rank.has_any());
    }

    #[test]
    fn error_code_40002_parses() {
        let resp: EasyScholarResponse =
            serde_json::from_str(r#"{ "code": 40002, "msg": "secretKey error" }"#).unwrap();
        assert_eq!(resp.code, 40002);
        assert_eq!(resp.msg.as_deref(), Some("secretKey error"));
    }
}
