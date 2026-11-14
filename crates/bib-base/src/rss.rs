//! RSS/Atom feed subscriptions: durable feeds, fetched items and a host-owned
//! scheduler. Items land in a local inbox and enter the library only through
//! an explicit user import.
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tokio::sync::Mutex;
use turso::{Connection, params};

pub const MIN_INTERVAL_SECONDS: i64 = 300;
pub const MAX_INTERVAL_SECONDS: i64 = 2_592_000;
pub const MAX_FEEDS: usize = 200;

#[derive(Debug, thiserror::Error)]
pub enum RssError {
    #[error("invalid feed: {0}")]
    Invalid(String),
    #[error("conflict: {0}")]
    Conflict(String),
    #[error("not found")]
    NotFound,
    #[error(transparent)]
    Store(#[from] turso::Error),
    #[error("feed fetch failed: {0}")]
    Fetch(String),
    #[error("feed parse failed: {0}")]
    Parse(String),
}
pub type Result<T> = std::result::Result<T, RssError>;

pub fn now_ms() -> i64 {
    Utc::now().timestamp_millis()
}

/// User-editable feed definition.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct FeedSpec {
    pub name: String,
    pub url: String,
    pub enabled: bool,
    pub interval_seconds: i64,
    /// Retain at most this many items per feed (oldest read items pruned).
    pub max_items_per_feed: i64,
}
impl Default for FeedSpec {
    fn default() -> Self {
        Self {
            name: String::new(),
            url: String::new(),
            enabled: true,
            interval_seconds: 3600,
            max_items_per_feed: 500,
        }
    }
}
impl FeedSpec {
    fn validate(&self) -> Result<()> {
        if self.name.trim().is_empty() || self.name.len() > 256 {
            return Err(RssError::Invalid("feed name must be 1–256 characters".into()));
        }
        let parsed = reqwest::Url::parse(self.url.trim())
            .map_err(|_| RssError::Invalid("feed URL is not a valid absolute URL".into()))?;
        if !matches!(parsed.scheme(), "http" | "https") {
            return Err(RssError::Invalid("feed URL must be http(s)".into()));
        }
        if !(MIN_INTERVAL_SECONDS..=MAX_INTERVAL_SECONDS).contains(&self.interval_seconds) {
            return Err(RssError::Invalid(
                "check interval must be 5 minutes – 30 days".into(),
            ));
        }
        if !(1..=5_000).contains(&self.max_items_per_feed) {
            return Err(RssError::Invalid("per-feed item retention must be 1–5000".into()));
        }
        Ok(())
    }
}

/// Stored feed with scheduling state.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Feed {
    pub id: String,
    pub version: u64,
    pub spec: FeedSpec,
    pub etag: Option<String>,
    pub last_modified: Option<String>,
    pub last_checked_ms: Option<i64>,
    pub last_success_ms: Option<i64>,
    pub last_error: Option<String>,
    pub next_due_ms: i64,
    pub item_count: i64,
    pub unread_count: i64,
}

/// One feed item waiting in the inbox.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FeedItem {
    pub id: String,
    pub feed_id: String,
    pub guid: String,
    pub title: String,
    pub link: Option<String>,
    pub summary: Option<String>,
    pub authors: Vec<String>,
    pub published_ms: Option<i64>,
    pub first_seen_ms: i64,
    pub read: bool,
    pub imported_article_id: Option<String>,
}

/// Result of one feed check.
#[derive(Debug, Clone, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CheckOutcome {
    pub feed_id: String,
    pub status: &'static str, // "fetched" | "not_modified" | "error"
    pub new_items: usize,
    pub total_items: usize,
    pub error: Option<String>,
}

// ---------------------------------------------------------------------------
// Storage
// ---------------------------------------------------------------------------

#[derive(Clone)]
pub struct RssStore {
    conn: Arc<Mutex<Connection>>,
}

impl RssStore {
    pub async fn open(path: &str) -> Result<Self> {
        let mut builder = turso::Builder::new_local(path);
        if path != ":memory:" {
            builder = builder.experimental_multiprocess_wal(true);
        }
        let db = builder.build().await.map_err(RssError::Store)?;
        let conn = db.connect()?;
        if path != ":memory:" {
            conn.pragma_update("busy_timeout", 5000).await?;
        }
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS rssFeeds(\
             id TEXT PRIMARY KEY, name TEXT NOT NULL, url TEXT NOT NULL UNIQUE,\
             enabled INTEGER NOT NULL, intervalSeconds INTEGER NOT NULL,\
             maxItemsPerFeed INTEGER NOT NULL, version INTEGER NOT NULL,\
             etag TEXT, lastModified TEXT, lastCheckedMs INTEGER, lastSuccessMs INTEGER,\
             lastError TEXT, nextDueMs INTEGER NOT NULL, createdAt TEXT NOT NULL);\
             CREATE TABLE IF NOT EXISTS rssItems(\
             seq INTEGER PRIMARY KEY AUTOINCREMENT, id TEXT NOT NULL UNIQUE,\
             feedId TEXT NOT NULL, guid TEXT NOT NULL, title TEXT NOT NULL,\
             link TEXT, summary TEXT, authors TEXT NOT NULL, publishedMs INTEGER,\
             firstSeenMs INTEGER NOT NULL, read INTEGER NOT NULL DEFAULT 0,\
             importedArticleId TEXT);\
             CREATE UNIQUE INDEX IF NOT EXISTS rssItemsFeedGuid ON rssItems(feedId,guid);\
             CREATE INDEX IF NOT EXISTS rssItemsFeedSeen ON rssItems(feedId,seq DESC);",
        )
        .await?;
        Ok(Self {
            conn: Arc::new(Mutex::new(conn)),
        })
    }

    pub async fn list_feeds(&self) -> Result<Vec<Feed>> {
        let conn = self.conn.lock().await;
        let mut rows = conn
            .query(
                "SELECT f.id,f.version,f.name,f.url,f.enabled,f.intervalSeconds,f.maxItemsPerFeed,\
                 f.etag,f.lastModified,f.lastCheckedMs,f.lastSuccessMs,f.lastError,f.nextDueMs,\
                 (SELECT COUNT(*) FROM rssItems i WHERE i.feedId=f.id),\
                 (SELECT COUNT(*) FROM rssItems i WHERE i.feedId=f.id AND i.read=0)\
                 FROM rssFeeds f ORDER BY f.createdAt",
                params![],
            )
            .await?;
        let mut feeds = Vec::new();
        while let Some(row) = rows.next().await? {
            feeds.push(feed_from_row(row)?);
        }
        drop(rows);
        Ok(feeds)
    }

    pub async fn get_feed(&self, id: &str) -> Result<Feed> {
        let conn = self.conn.lock().await;
        let mut rows = conn
            .query(
                "SELECT f.id,f.version,f.name,f.url,f.enabled,f.intervalSeconds,f.maxItemsPerFeed,\
                 f.etag,f.lastModified,f.lastCheckedMs,f.lastSuccessMs,f.lastError,f.nextDueMs,\
                 (SELECT COUNT(*) FROM rssItems i WHERE i.feedId=f.id),\
                 (SELECT COUNT(*) FROM rssItems i WHERE i.feedId=f.id AND i.read=0)\
                 FROM rssFeeds f WHERE f.id=?1",
                params![id],
            )
            .await?;
        let Some(row) = rows.next().await? else {
            return Err(RssError::NotFound);
        };
        let feed = feed_from_row(row)?;
        drop(rows);
        Ok(feed)
    }

    pub async fn create_feed(&self, spec: FeedSpec) -> Result<Feed> {
        spec.validate()?;
        let id = uuid::Uuid::new_v4().to_string();
        let now = now_ms();
        let conn = self.conn.lock().await;
        let mut rows = conn
            .query("SELECT COUNT(*) FROM rssFeeds", params![])
            .await?;
        let count: i64 = rows.next().await?.unwrap().get(0)?;
        drop(rows);
        if count as usize >= MAX_FEEDS {
            return Err(RssError::Invalid(format!(
                "at most {MAX_FEEDS} feeds are supported"
            )));
        }
        let mut rows = conn
            .query(
                "SELECT COUNT(*) FROM rssFeeds WHERE url=?1",
                params![spec.url.trim()],
            )
            .await?;
        let exists: i64 = rows.next().await?.unwrap().get(0)?;
        drop(rows);
        if exists > 0 {
            return Err(RssError::Conflict("a feed with this URL already exists".into()));
        }
        conn.execute(
            "INSERT INTO rssFeeds(id,name,url,enabled,intervalSeconds,maxItemsPerFeed,version,\
             nextDueMs,createdAt) VALUES (?1,?2,?3,?4,?5,?6,0,?7,?8)",
            params![
                id.as_str(),
                spec.name.trim(),
                spec.url.trim(),
                i64::from(spec.enabled),
                spec.interval_seconds,
                spec.max_items_per_feed,
                now,
                Utc::now().to_rfc3339()
            ],
        )
        .await?;
        drop(conn);
        self.get_feed(&id).await
    }

    pub async fn replace_feed(&self, id: &str, version: u64, spec: FeedSpec) -> Result<Feed> {
        spec.validate()?;
        let conn = self.conn.lock().await;
        let changed = conn
            .execute(
                "UPDATE rssFeeds SET name=?2,url=?3,enabled=?4,intervalSeconds=?5,\
                 maxItemsPerFeed=?6,version=version+1 WHERE id=?1 AND version=?7",
                params![
                    id,
                    spec.name.trim(),
                    spec.url.trim(),
                    i64::from(spec.enabled),
                    spec.interval_seconds,
                    spec.max_items_per_feed,
                    version as i64
                ],
            )
            .await?;
        drop(conn);
        if changed == 0 {
            return Err(self.stale(id, version).await);
        }
        self.get_feed(id).await
    }

    pub async fn set_enabled(&self, id: &str, version: u64, enabled: bool) -> Result<Feed> {
        let conn = self.conn.lock().await;
        let changed = conn
            .execute(
                "UPDATE rssFeeds SET enabled=?2,version=version+1 WHERE id=?1 AND version=?3",
                params![id, i64::from(enabled), version as i64],
            )
            .await?;
        drop(conn);
        if changed == 0 {
            return Err(self.stale(id, version).await);
        }
        self.get_feed(id).await
    }

    pub async fn delete_feed(&self, id: &str, version: u64) -> Result<()> {
        let conn = self.conn.lock().await;
        let changed = conn
            .execute(
                "DELETE FROM rssFeeds WHERE id=?1 AND version=?2",
                params![id, version as i64],
            )
            .await?;
        if changed > 0 {
            conn.execute("DELETE FROM rssItems WHERE feedId=?1", params![id])
                .await?;
        }
        drop(conn);
        if changed == 0 {
            return Err(self.stale(id, version).await);
        }
        Ok(())
    }

    async fn stale(&self, id: &str, version: u64) -> RssError {
        match self.get_feed(id).await {
            Ok(_) => RssError::Conflict(format!(
                "feed was modified concurrently (expected version {version})"
            )),
            Err(_) => RssError::NotFound,
        }
    }

    /// Feeds whose next check is due, capped for one scheduler pass.
    pub async fn due_feeds(&self, now: i64, limit: usize) -> Result<Vec<Feed>> {
        let conn = self.conn.lock().await;
        let mut rows = conn
            .query(
                "SELECT f.id,f.version,f.name,f.url,f.enabled,f.intervalSeconds,f.maxItemsPerFeed,\
                 f.etag,f.lastModified,f.lastCheckedMs,f.lastSuccessMs,f.lastError,f.nextDueMs,\
                 (SELECT COUNT(*) FROM rssItems i WHERE i.feedId=f.id),\
                 (SELECT COUNT(*) FROM rssItems i WHERE i.feedId=f.id AND i.read=0)\
                 FROM rssFeeds f WHERE f.enabled=1 AND f.nextDueMs<=?1 ORDER BY f.nextDueMs LIMIT ?2",
                params![now, limit as i64],
            )
            .await?;
        let mut feeds = Vec::new();
        while let Some(row) = rows.next().await? {
            feeds.push(feed_from_row(row)?);
        }
        drop(rows);
        Ok(feeds)
    }

    /// Record the outcome of one check and insert any new items.
    pub async fn commit_check(
        &self,
        feed: &Feed,
        outcome: &CheckOutcome,
        items: &[ParsedItem],
    ) -> Result<()> {
        let conn = self.conn.lock().await;
        for item in items {
            conn.execute(
                "INSERT OR IGNORE INTO rssItems\
                 (id,feedId,guid,title,link,summary,authors,publishedMs,firstSeenMs,read)\
                 VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,0)",
                params![
                    uuid::Uuid::new_v4().to_string(),
                    feed.id.as_str(),
                    item.guid.as_str(),
                    item.title.as_str(),
                    item.link.as_deref(),
                    item.summary.as_deref(),
                    serde_json::to_string(&item.authors).unwrap_or_else(|_| "[]".into()),
                    item.published_ms,
                    now_ms()
                ],
            )
            .await?;
        }
        // Prune oldest read items beyond the per-feed budget: drop read items
        // older than the newest `max_items_per_feed` retained rows.
        conn.execute(
            "DELETE FROM rssItems WHERE feedId=?1 AND read=1 AND seq < COALESCE((\
             SELECT seq FROM rssItems WHERE feedId=?1 ORDER BY seq DESC LIMIT 1 OFFSET ?2),0)",
            params![feed.id.as_str(), feed.spec.max_items_per_feed.saturating_sub(1)],
        )
        .await?;
        conn.execute(
            "UPDATE rssFeeds SET etag=?2,lastModified=?3,lastCheckedMs=?4,lastSuccessMs=?5,\
             lastError=NULL,nextDueMs=?4+?6 WHERE id=?1",
            params![
                feed.id.as_str(),
                feed.etag.as_deref(),
                feed.last_modified.as_deref(),
                now_ms(),
                if outcome.status != "error" { Some(now_ms()) } else { None },
                feed.spec.interval_seconds
            ],
        )
        .await?;
        if let Some(error) = &outcome.error {
            conn.execute(
                "UPDATE rssFeeds SET lastError=?2 WHERE id=?1",
                params![feed.id.as_str(), error.as_str()],
            )
            .await?;
        }
        drop(conn);
        Ok(())
    }

    pub async fn items(
        &self,
        feed_id: Option<&str>,
        before_seq: Option<i64>,
        limit: u16,
    ) -> Result<(Vec<FeedItem>, Option<i64>)> {
        let conn = self.conn.lock().await;
        let sql = match (feed_id, before_seq) {
            (Some(_), Some(_)) => "SELECT seq,id,feedId,guid,title,link,summary,authors,publishedMs,firstSeenMs,read,importedArticleId FROM rssItems WHERE feedId=?1 AND seq<?2 ORDER BY seq DESC LIMIT ?3",
            (Some(_), None) => "SELECT seq,id,feedId,guid,title,link,summary,authors,publishedMs,firstSeenMs,read,importedArticleId FROM rssItems WHERE feedId=?1 ORDER BY seq DESC LIMIT ?3",
            (None, Some(_)) => "SELECT seq,id,feedId,guid,title,link,summary,authors,publishedMs,firstSeenMs,read,importedArticleId FROM rssItems WHERE seq<?2 ORDER BY seq DESC LIMIT ?3",
            (None, None) => "SELECT seq,id,feedId,guid,title,link,summary,authors,publishedMs,firstSeenMs,read,importedArticleId FROM rssItems ORDER BY seq DESC LIMIT ?3",
        };
        let mut rows = conn
            .query(sql, params![feed_id, before_seq, limit as i64 + 1])
            .await?;
        let mut items = Vec::new();
        let mut next = None;
        while let Some(row) = rows.next().await? {
            if items.len() >= limit as usize {
                next = Some(row.get::<i64>(0)?);
                break;
            }
            items.push(item_from_row(row)?);
        }
        drop(rows);
        Ok((items, next))
    }

    pub async fn get_item(&self, id: &str) -> Result<FeedItem> {
        let conn = self.conn.lock().await;
        let mut rows = conn
            .query(
                "SELECT seq,id,feedId,guid,title,link,summary,authors,publishedMs,firstSeenMs,read,\
                 importedArticleId FROM rssItems WHERE id=?1",
                params![id],
            )
            .await?;
        let Some(row) = rows.next().await? else {
            return Err(RssError::NotFound);
        };
        let item = item_from_row(row)?;
        drop(rows);
        Ok(item)
    }

    pub async fn set_item_read(&self, id: &str, read: bool) -> Result<()> {
        let conn = self.conn.lock().await;
        let changed = conn
            .execute("UPDATE rssItems SET read=?2 WHERE id=?1", params![id, i64::from(read)])
            .await?;
        drop(conn);
        if changed == 0 {
            return Err(RssError::NotFound);
        }
        Ok(())
    }

    pub async fn delete_item(&self, id: &str) -> Result<()> {
        let conn = self.conn.lock().await;
        let changed = conn.execute("DELETE FROM rssItems WHERE id=?1", params![id]).await?;
        drop(conn);
        if changed == 0 {
            return Err(RssError::NotFound);
        }
        Ok(())
    }

    /// Mark an item imported by attaching the created article id.
    pub async fn mark_imported(&self, id: &str, article_id: &str) -> Result<()> {
        let conn = self.conn.lock().await;
        conn.execute(
            "UPDATE rssItems SET importedArticleId=?2,read=1 WHERE id=?1",
            params![id, article_id],
        )
        .await?;
        drop(conn);
        Ok(())
    }

    /// All guids currently stored for one feed (dedup set).
    pub async fn known_guids(&self, feed_id: &str) -> Result<std::collections::HashSet<String>> {
        let conn = self.conn.lock().await;
        let mut rows = conn
            .query(
                "SELECT guid FROM rssItems WHERE feedId=?1",
                params![feed_id],
            )
            .await?;
        let mut guids = std::collections::HashSet::new();
        while let Some(row) = rows.next().await? {
            guids.insert(row.get::<String>(0)?);
        }
        drop(rows);
        Ok(guids)
    }

    pub async fn total_items(&self) -> Result<i64> {
        let conn = self.conn.lock().await;
        let mut rows = conn
            .query("SELECT COUNT(*) FROM rssItems", params![])
            .await?;
        let count: i64 = rows.next().await?.unwrap().get(0)?;
        drop(rows);
        drop(conn);
        Ok(count)
    }
}

fn opt_string(value: Option<String>) -> Option<String> {
    value.filter(|s| !s.is_empty())
}

fn feed_from_row(row: turso::Row) -> Result<Feed> {
    Ok(Feed {
        id: row.get(0)?,
        version: row.get::<i64>(1)? as u64,
        spec: FeedSpec {
            name: row.get(2)?,
            url: row.get(3)?,
            enabled: row.get::<i64>(4)? != 0,
            interval_seconds: row.get(5)?,
            max_items_per_feed: row.get(6)?,
        },
        etag: opt_string(row.get(7)?),
        last_modified: opt_string(row.get(8)?),
        last_checked_ms: row.get(9)?,
        last_success_ms: row.get(10)?,
        last_error: opt_string(row.get(11)?),
        next_due_ms: row.get(12)?,
        item_count: row.get(13)?,
        unread_count: row.get(14)?,
    })
}

fn item_from_row(row: turso::Row) -> Result<FeedItem> {
    let authors_raw: String = row.get(7)?;
    Ok(FeedItem {
        id: row.get(1)?,
        feed_id: row.get(2)?,
        guid: row.get(3)?,
        title: row.get(4)?,
        link: opt_string(row.get(5)?),
        summary: opt_string(row.get(6)?),
        authors: serde_json::from_str(&authors_raw).unwrap_or_default(),
        published_ms: row.get(8)?,
        first_seen_ms: row.get(9)?,
        read: row.get::<i64>(10)? != 0,
        imported_article_id: opt_string(row.get(11)?),
    })
}

// ---------------------------------------------------------------------------
// Fetch + parse (RSS 2.0 and Atom)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct ParsedItem {
    pub guid: String,
    pub title: String,
    pub link: Option<String>,
    pub summary: Option<String>,
    pub authors: Vec<String>,
    pub published_ms: Option<i64>,
}

#[derive(Debug)]
pub struct FetchedFeed {
    pub etag: Option<String>,
    pub last_modified: Option<String>,
    pub items: Vec<ParsedItem>,
}

pub async fn fetch_feed(
    http: &reqwest::Client,
    url: &str,
    etag: Option<&str>,
    last_modified: Option<&str>,
) -> Result<Option<FetchedFeed>> {
    let mut request = http.get(url).timeout(std::time::Duration::from_secs(30));
    if let Some(etag) = etag {
        request = request.header(reqwest::header::IF_NONE_MATCH, etag);
    }
    if let Some(lm) = last_modified {
        request = request.header(reqwest::header::IF_MODIFIED_SINCE, lm);
    }
    let response = request
        .send()
        .await
        .map_err(|error| RssError::Fetch(error.to_string()))?;
    if response.status() == reqwest::StatusCode::NOT_MODIFIED {
        return Ok(None);
    }
    if !response.status().is_success() {
        return Err(RssError::Fetch(format!("feed returned {}", response.status())));
    }
    let etag = response
        .headers()
        .get(reqwest::header::ETAG)
        .and_then(|v| v.to_str().ok())
        .map(str::to_owned);
    let last_modified = response
        .headers()
        .get(reqwest::header::LAST_MODIFIED)
        .and_then(|v| v.to_str().ok())
        .map(str::to_owned);
    let body = response
        .text()
        .await
        .map_err(|error| RssError::Fetch(error.to_string()))?;
    let items = parse_feed(&body)?;
    Ok(Some(FetchedFeed {
        etag,
        last_modified,
        items,
    }))
}

/// Minimal streaming parser for RSS 2.0 `<item>` and Atom `<entry>` elements.
/// quick-xml is already a workspace dependency.
pub fn parse_feed(body: &str) -> Result<Vec<ParsedItem>> {
    use quick_xml::events::Event;
    use quick_xml::Reader;
    let mut reader = Reader::from_str(body);
    let mut items = Vec::new();
    let mut stack: Vec<String> = Vec::new();
    let mut in_item = false;
    let mut field: Option<String> = None;
    let mut text = String::new();
    let mut current = Some(ItemBuilder::default());
    let mut buf = Vec::new();
    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(event)) => {
                let name = String::from_utf8_lossy(event.name().as_ref()).into_owned();
                if !in_item && (name == "item" || name == "entry") {
                    in_item = true;
                    current = Some(ItemBuilder::default());
                    stack.push(name);
                } else if in_item {
                    let lower = name.to_lowercase();
                    field = match lower.as_str() {
                        "title" | "link" | "guid" | "id" | "description" | "summary"
                        | "content" | "author" | "pubdate" | "published" | "updated"
                        | "dc:creator" => Some(lower.clone()),
                        _ => {
                            if lower == "name" && stack.last().is_some_and(|p| p == "author") {
                                Some(lower.clone())
                            } else {
                                None
                            }
                        }
                    };
                    // Atom links carry the URL in an href attribute.
                    if lower == "link" && field.is_some() {
                        for attr in event.attributes().flatten() {
                            if attr.key.as_ref() == b"href"
                                && current.as_ref().is_some_and(|c| c.link.is_none())
                                && !stack
                                    .last()
                                    .is_some_and(|p| p == "author" || p == "feed")
                                && stack.last().is_some_and(|p| p == "entry")
                            {
                                if let Some(c) = current.as_mut() {
                                    c.link = Some(
                                        String::from_utf8_lossy(&attr.value).into_owned(),
                                    );
                                }
                            }
                        }
                    }
                    text.clear();
                    stack.push(name);
                }
            }
            Ok(Event::Text(event)) => {
                if in_item && field.is_some() {
                    text.push_str(&event.decode().unwrap_or_default());
                }
            }
            Ok(Event::GeneralRef(event)) => {
                // Re-append the raw entity so a single unescape at field end
                // resolves it (`&amp;` → `&`, `&#8212;` → `—`).
                if in_item && field.is_some() {
                    text.push('&');
                    text.push_str(&String::from_utf8_lossy(event.as_ref()));
                    text.push(';');
                }
            }
            Ok(Event::CData(event)) => {
                if in_item && field.is_some() {
                    text.push_str(&String::from_utf8_lossy(event.as_ref()));
                }
            }
            Ok(Event::End(_)) => {
                if in_item {
                    if let Some(field) = field.take() {
                        let value = quick_xml::escape::unescape(text.trim())
                            .unwrap_or_default()
                            .into_owned();
                        if !value.is_empty() {
                            match field.as_str() {
                                "title" => {
                                    if let Some(c) = current.as_mut() { c.title.get_or_insert(value); }
                                }
                                "link" => {
                                    if let Some(c) = current.as_mut() { c.link.get_or_insert(value); }
                                }
                                "guid" | "id" => {
                                    if let Some(c) = current.as_mut() { c.guid.get_or_insert(value); }
                                }
                                "description" | "summary" | "content" => {
                                    if let Some(c) = current.as_mut() { c.summary.get_or_insert(strip_html(&value)); }
                                }
                                "author" | "dc:creator" | "name" => {
                                    if let Some(c) = current.as_mut() { c.authors.push(split_author(&value)); }
                                }
                                "pubdate" | "published" | "updated" => {
                                    if let Some(c) = current.as_mut() { c.published_ms.get_or_insert(parse_time(&value)); }
                                }
                                _ => {}
                            }
                        }
                    }
                }
                if let Some(name) = stack.pop() {
                    if name == "item" || name == "entry" {
                        in_item = false;
                        if let Some(builder) = current.take() {
                            if let Some(item) = builder.build() {
                                items.push(item);
                            }
                        }
                    }
                }
                text.clear();
            }
            Ok(Event::Empty(event)) => {
                // Self-closing elements — most importantly Atom
                // `<link href="…"/>`.
                let name = String::from_utf8_lossy(event.name().as_ref()).into_owned();
                if in_item && name.to_lowercase() == "link" && stack.last().is_some_and(|p| p == "entry")
                {
                    for attr in event.attributes().flatten() {
                        if attr.key.as_ref() == b"href"
                            && current.as_ref().is_some_and(|c| c.link.is_none())
                        {
                            if let Some(c) = current.as_mut() {
                                c.link =
                                    Some(String::from_utf8_lossy(&attr.value).into_owned());
                            }
                        }
                    }
                }
            }
            Ok(Event::Eof) => break,
            Ok(_) => {}
            Err(error) => return Err(RssError::Parse(error.to_string())),
        }
        buf.clear();
    }
    Ok(items)
}

#[derive(Default)]
struct ItemBuilder {
    title: Option<String>,
    link: Option<String>,
    guid: Option<String>,
    summary: Option<String>,
    authors: Vec<String>,
    published_ms: Option<i64>,
}
impl ItemBuilder {
    fn build(self) -> Option<ParsedItem> {
        let title = self.title?;
        Some(ParsedItem {
            guid: self
                .guid
                .or_else(|| self.link.clone())
                .unwrap_or_else(|| title.clone()),
            title,
            link: self.link,
            summary: self.summary,
            authors: self.authors,
            published_ms: self.published_ms,
        })
    }
}

fn strip_html(value: &str) -> String {
    // Good enough for summaries: drop tags and collapse whitespace.
    let mut out = String::with_capacity(value.len());
    let mut in_tag = false;
    for ch in value.chars() {
        match ch {
            '<' => in_tag = true,
            '>' => in_tag = false,
            c if !in_tag => out.push(c),
            _ => {}
        }
    }
    out.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// RSS/Atom author strings arrive as "Family, Given" or "Given Family".
fn split_author(value: &str) -> String {
    value.trim().to_owned()
}

fn parse_time(value: &str) -> i64 {
    DateTime::parse_from_rfc2822(value)
        .or_else(|_| DateTime::parse_from_rfc3339(value))
        .map(|t| t.timestamp_millis())
        .unwrap_or(0)
}

// ---------------------------------------------------------------------------
// Check + import
// ---------------------------------------------------------------------------

/// Fetch one feed now and persist new items.
pub async fn check_feed(shared: &crate::shared::BibShared, feed_id: &str) -> Result<CheckOutcome> {
    let mut feed = shared.rss.get_feed(feed_id).await?;
    let fetched = match fetch_feed(
        &shared.http,
        &feed.spec.url,
        feed.etag.as_deref(),
        feed.last_modified.as_deref(),
    )
    .await
    {
        Ok(Some(fetched)) => fetched,
        Ok(None) => {
            let outcome = CheckOutcome {
                feed_id: feed.id.clone(),
                status: "not_modified",
                new_items: 0,
                total_items: feed.item_count as usize,
                error: None,
            };
            shared.rss.commit_check(&feed, &outcome, &[]).await?;
            return Ok(outcome);
        }
        Err(error) => {
            let outcome = CheckOutcome {
                feed_id: feed.id.clone(),
                status: "error",
                new_items: 0,
                total_items: feed.item_count as usize,
                error: Some(error.to_string()),
            };
            shared.rss.commit_check(&feed, &outcome, &[]).await?;
            return Err(error);
        }
    };
    feed.etag = fetched.etag;
    feed.last_modified = fetched.last_modified;
    let known = shared.rss.known_guids(&feed.id).await?;
    let fresh: Vec<ParsedItem> = fetched
        .items
        .into_iter()
        .filter(|item| !known.contains(&item.guid))
        .collect();
    let new_items = fresh.len();
    let outcome = CheckOutcome {
        feed_id: feed.id.clone(),
        status: "fetched",
        new_items,
        total_items: feed.item_count as usize + new_items,
        error: None,
    };
    shared.rss.commit_check(&feed, &outcome, &fresh).await?;
    Ok(outcome)
}

static DOI_RE: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
    regex::Regex::new(r#"10\.\d{4,9}/[^\s"<>]+"#).unwrap()
});
static ARXIV_RE: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
    regex::Regex::new(r"arxiv\.org/abs/([0-9]{4}\.[0-9]{4,5}|[a-z-]+/[0-9]{7})").unwrap()
});

fn identifiers_from_item(item: &FeedItem) -> Vec<bib_types::Identifier> {
    use bib_types::{IdKind, Identifier};
    let mut ids = Vec::new();
    let haystacks = [
        item.link.as_deref().unwrap_or(""),
        &item.guid,
        item.summary.as_deref().unwrap_or(""),
    ];
    for text in haystacks {
        if ids.is_empty() {
            if let Some(matched) = DOI_RE.find(text) {
                ids.push(Identifier::doi(matched.as_str().trim_end_matches(['.', ')', ','])));
            }
        }
        if ids.is_empty() {
            if let Some(matched) = ARXIV_RE.captures(text) {
                ids.push(Identifier::new(
                    IdKind::Arxiv,
                    matched.get(1).unwrap().as_str(),
                ));
            }
        }
    }
    ids
}

/// Build a library article from one feed item. Item metadata is imported as-is;
/// dedup identifiers are only added when a DOI or arXiv id is visible.
pub fn article_from_item(item: &FeedItem) -> bib_types::Article {
    let mut article = bib_types::Article::new(
        uuid::Uuid::new_v4().to_string(),
        item.title.trim().to_owned(),
    );
    article.abstract_text = item.summary.clone().filter(|s| !s.is_empty());
    for author in &item.authors {
        let (last, fore) = match author.split_once(',') {
            Some((last, fore)) => (last.trim(), Some(fore.trim())),
            None => match author.rsplit_once(' ') {
                Some((fore, last)) => (last.trim(), Some(fore.trim())),
                None => (author.trim(), None),
            },
        };
        if last.is_empty() {
            continue;
        }
        article.authors.push(bib_types::Author {
            last_name: last.to_owned(),
            fore_name: fore.filter(|s| !s.is_empty()).map(str::to_owned),
            initials: None,
            affiliation: None,
            orcid: None,
            corresponding: false,
        });
    }
    article.identifiers = identifiers_from_item(item);
    if let Some(published) = item.published_ms.filter(|ms| *ms > 0) {
        let date = DateTime::<Utc>::from_timestamp_millis(published).map(|dt| dt.date_naive());
        if let Some(date) = date {
            use chrono::Datelike;
            article.year = Some(i64::from(date.year()).clamp(0, 9999) as u16);
            article.month = Some(date.month() as u8);
        }
    }
    // The item link is kept as an `Other` identifier so the record stays
    // traceable back to its feed entry.
    if let Some(link) = &item.link {
        article.identifiers.push(bib_types::Identifier::new(
            bib_types::IdKind::Other,
            link.clone(),
        ));
    }
    article
}

// ---------------------------------------------------------------------------
// Host-owned scheduler
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SchedulerStatus {
    pub running: bool,
    pub active_checks: usize,
    pub last_error: Option<String>,
}

#[derive(Clone)]
pub struct RssScheduler {
    shutdown: Arc<tokio::sync::Notify>,
    status: Arc<std::sync::Mutex<SchedulerStatus>>,
}

impl RssScheduler {
    /// Start the host-owned background loop. Checks continue while GUI views
    /// are closed; dropping every handle stops the workers.
    pub fn start(shared: crate::shared::BibShared, concurrency: usize) -> Self {
        let scheduler = Self {
            shutdown: Arc::new(tokio::sync::Notify::new()),
            status: Arc::new(std::sync::Mutex::new(SchedulerStatus {
                running: true,
                active_checks: 0,
                last_error: None,
            })),
        };
        let shutdown = scheduler.shutdown.clone();
        let status = scheduler.status.clone();
        tokio::spawn(async move {
            let semaphore = Arc::new(tokio::sync::Semaphore::new(concurrency.max(1)));
            loop {
                let mut errors = Vec::new();
                let due = match shared.rss.due_feeds(now_ms(), 50).await {
                    Ok(due) => due,
                    Err(error) => {
                        errors.push(error.to_string());
                        Vec::new()
                    }
                };
                let mut workers = Vec::new();
                for feed in due {
                    let permit = semaphore.clone();
                    let shared = shared.clone();
                    let status = status.clone();
                    workers.push(tokio::spawn(async move {
                        let _guard = permit.acquire_owned().await;
                        if let Ok(mut current) = status.lock() {
                            current.active_checks += 1;
                        }
                        let result = check_feed(&shared, &feed.id).await;
                        if let Ok(mut current) = status.lock() {
                            current.active_checks -= 1;
                        }
                        // Fetch/parse errors are persisted on the subscription. They
                        // are not scheduler failures; deletion during a check is normal.
                        match result {
                            Err(error @ RssError::Store(_)) => Some(error.to_string()),
                            _ => None,
                        }
                    }));
                }
                for worker in workers {
                    match worker.await {
                        Ok(Some(error)) => errors.push(error),
                        Err(error) => errors.push(format!("RSS worker failed: {error}")),
                        Ok(None) => {}
                    }
                }
                if let Ok(mut current) = status.lock() {
                    current.active_checks = 0;
                    current.last_error = if errors.is_empty() { None } else { Some(errors.join("; ")) };
                }
                if tokio::select! {
                    _ = shutdown.notified() => true,
                    _ = tokio::time::sleep(std::time::Duration::from_secs(30)) => false,
                } {
                    break;
                }
            }
            if let Ok(mut current) = status.lock() {
                current.running = false;
            }
        });
        scheduler
    }

    pub fn status(&self) -> SchedulerStatus {
        self.status.lock().map(|s| s.clone()).unwrap_or(SchedulerStatus {
            running: false,
            active_checks: 0,
            last_error: Some("scheduler state unavailable".into()),
        })
    }

    pub async fn shutdown(&self) {
        self.shutdown.notify_waiters();
    }
}
