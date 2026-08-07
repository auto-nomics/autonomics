//! Process-level shared bibliography infrastructure.
//!
//! [`BibShared`] aggregates the storage handle ([`BibBase`]), the
//! literature query gateway ([`LiteratureGateway`]) **and** the underlying
//! HTTP clients / rate-limiters used by the gateway's sources into a
//! single cheaply-cloneable bundle.
//!
//! Why fold the HTTP clients in here?
//!
//! Without this, every [`BibShared::open`] call would also call
//! `EutilsClient::from_env`, `ArxivClient::new`, and `reqwest::Client::new`
//! afresh. In a multi-agent network this means N independent reqwest
//! connection pools, N independent arXiv rate-limit windows
//! (`ArxivClient` keeps a `Mutex<Option<Instant>>` for the 3 s arXiv
//! policy), and N independent `EuropePmcClient`s inside the library
//! tool registrations. Folding the clients into `BibShared` means the
//! process has **one** shared connection pool and **one** shared rate
//! limiter regardless of how many agents run.
//!
//! `BibShared` is `Clone` and every field is `Arc`-backed, so handing a
//! clone to each spawned agent is cheap.

use std::sync::Arc;

use crate::bib_base::BibBase;
use crate::query::LiteratureGateway;
use crate::Result;

/// Build the shared `reqwest::Client` from [`BibHttpOptions`]. Used by
/// `BiorxivSource` and any future source that wants a bare HTTP client.
fn http_client_from(opts: crate::BibHttpOptions) -> Arc<reqwest::Client> {
    Arc::new(opts.build_client())
}

/// Aggregated, process-level bibliography handle. Cheap to clone
/// (`Arc`-backed).
#[derive(Clone)]
pub struct BibShared {
    /// Storage handle for the bibliography library (articles, collections,
    /// fulltexts, annotations, …).
    pub bib: Arc<BibBase>,
    /// Multi-source literature search/fetch gateway.
    ///
    /// Built with the shared `eutils` / `arxiv` / `http` clients below so
    /// every `Arc<LiteratureGateway>` clone shares a single connection
    /// pool and a single arXiv rate-limit window.
    pub gateway: Arc<LiteratureGateway>,

    /// Shared NCBI E-utilities (PubMed) client.
    pub eutils: Arc<eutils::EutilsClient>,
    /// Shared arXiv API client. Its internal `last_request` mutex is what
    /// enforces the 3-second arXiv rate limit; sharing one instance
    /// process-wide means that limit is enforced globally instead of
    /// per-agent.
    pub arxiv: Arc<arxiv::ArxivClient>,
    /// Shared raw `reqwest::Client` used by `BiorxivSource` (and any
    /// future source that wants a bare HTTP client).
    pub http: Arc<reqwest::Client>,
    /// Shared Europe PMC client used by the `bib_save` tool's
    /// open-access full-text auto-fetch.
    pub europe_pmc: Arc<europepmc::EuropePmcClient>,
}

impl BibShared {
    /// Open (or create) the bibliography DB at `path`, pair it with a
    /// gateway pre-loaded with all built-in literature sources, and
    /// allocate the shared HTTP clients. **All resources are opened
    /// exactly once here**; subsequent `Clone`s share the same handles.
    ///
    /// Accepts anything that can be referenced as a path (`&str`,
    /// `String`, `PathBuf`, `&Path`) for caller ergonomics.
    /// Open (or create) the bibliography DB at `path` and pair it with
    /// a gateway pre-loaded with all built-in literature sources. Uses
    /// the default [`BibHttpOptions`](crate::BibHttpOptions) for the
    /// shared HTTP client. Use [`open_with`](Self::open_with) when you
    /// need custom timeouts, proxy, or user agent.
    pub async fn open(path: impl AsRef<std::path::Path>) -> Result<Self> {
        Self::open_with(path, crate::BibHttpOptions::default()).await
    }

    /// Open (or create) the bibliography DB at `path` and pair it with
    /// a gateway pre-loaded with all built-in literature sources. The
    /// `http_opts` configure the shared `reqwest::Client` (user agent,
    /// timeouts, proxy, TLS). All resources are opened **exactly once
    /// here**; subsequent `Clone`s share the same handles.
    pub async fn open_with(
        path: impl AsRef<std::path::Path>,
        http_opts: crate::BibHttpOptions,
    ) -> Result<Self> {
        let bib = Arc::new(BibBase::open(path.as_ref().to_string_lossy().as_ref()).await?);
        let eutils = Arc::new(eutils::EutilsClient::from_env());
        let arxiv = Arc::new(arxiv::ArxivClient::new());
        let http = http_client_from(http_opts);
        let europe_pmc = Arc::new(europepmc::EuropePmcClient::new());
        let gateway = Arc::new(LiteratureGateway::with_shared_clients(
            eutils.clone(),
            arxiv.clone(),
            http.clone(),
        ));
        Ok(Self {
            bib,
            gateway,
            eutils,
            arxiv,
            http,
            europe_pmc,
        })
    }

    /// Open an in-memory shared bundle (useful for tests) with default
    /// HTTP options.
    pub async fn open_in_memory() -> Result<Self> {
        Self::open_in_memory_with(crate::BibHttpOptions::default()).await
    }

    /// Open an in-memory shared bundle (useful for tests) with custom
    /// HTTP options.
    pub async fn open_in_memory_with(http_opts: crate::BibHttpOptions) -> Result<Self> {
        let bib = Arc::new(BibBase::open_in_memory().await?);
        let eutils = Arc::new(eutils::EutilsClient::from_env());
        let arxiv = Arc::new(arxiv::ArxivClient::new());
        let http = http_client_from(http_opts);
        let europe_pmc = Arc::new(europepmc::EuropePmcClient::new());
        let gateway = Arc::new(LiteratureGateway::with_shared_clients(
            eutils.clone(),
            arxiv.clone(),
            http.clone(),
        ));
        Ok(Self {
            bib,
            gateway,
            eutils,
            arxiv,
            http,
            europe_pmc,
        })
    }
}

impl std::fmt::Debug for BibShared {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BibShared")
            .field("bib", &"Arc<BibBase>")
            .field("gateway", &"Arc<LiteratureGateway>")
            .field("eutils", &"Arc<EutilsClient>")
            .field("arxiv", &"Arc<ArxivClient>")
            .field("http", &"Arc<reqwest::Client>")
            .field("europe_pmc", &"Arc<EuropePmcClient>")
            .finish()
    }
}
