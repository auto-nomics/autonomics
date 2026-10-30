//! Turso 后端:工作区既定的嵌入式打开方式 + 手写行解析。
//!
//! 与 kms / writing-base / bib-base 同款形态:`Builder::new_local` 打开,
//! 非内存库启用 `experimental_multiprocess_wal`(daemon 与 TUI 并发持库)
//! 并设 5 s busy timeout;连接收进 `tokio::sync::Mutex` 串行化访问;多
//! 语句写路径用显式 BEGIN/COMMIT 包裹,失败 ROLLBACK。行解析手写 ——
//! turso 不提供 `FromRow` 派生(与 kms 相同的取舍)。
//!
//! 写路径单事务维护四套衍生数据,与词条行保持原子:
//! - 全文倒排:`search_postings`/`search_docs`(turso 无 FTS,同
//!   bib-base 自建);
//! - 别名/标签索引:`aliases`/`tags`(别名可作 wiki 链接目标与提及
//!   来源,标签供检索过滤);
//! - 出边:`entry_links`,由 content 中的 wiki 链接(`[[...]]`)经标题/
//!   别名解析生成(见 [`crate::markdown`]);悬空目标不入库,自指边跳过;
//! - 版本快照:`entry_versions`,每次写全量快照(writing-base 同款)。
//!
//! 软删除:回收站词条(`deleted_at` 非空)从读取/检索/邻接中隐去,但
//! 继续占用标题;索引行原样保留,`restore_entry` 即恢复。`purge_entry`
//! 才真正删除(含全部衍生行)并释放标题。

use std::collections::{HashMap, HashSet};
use std::future::Future;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::Arc;

use async_trait::async_trait;
use tokio::sync::Mutex;
use turso::{Connection, Value};

use super::blob::BlobStore;
use super::error::{Error, Result};
use super::types::{
    AttachmentMeta, EntryVersionSummary, KnowledgeEntryData, ListEntrySummary, ListFilter,
    RecycleItem, SearchFilter, SearchHit, WikiAttachmentStorage, WikiEntryStorage,
    WikiIndexStorage, WikiSearchStorage,
};
use crate::markdown::{LinkKind, Parser};
use crate::search::{bm25_term, idf, tokenize};
use sha2::{Digest, Sha256};

/// `IN (...)` 查询单块的参数上限(SQLite 系绑定变量上限 999;80 与
/// bib-base 的分块粒度一致)。
const IN_CHUNK: usize = 80;

const SCHEMA_SQL: &str = "\
CREATE TABLE IF NOT EXISTS entries (
    id         TEXT PRIMARY KEY NOT NULL,
    title      TEXT NOT NULL,
    alias      TEXT NOT NULL DEFAULT '[]',
    content    TEXT NOT NULL,
    ctime      INTEGER NOT NULL,
    mtime      INTEGER NOT NULL,
    deleted_at INTEGER,
    UNIQUE(title)
);

CREATE TABLE IF NOT EXISTS entry_links (
    from_id TEXT NOT NULL,
    to_id   TEXT NOT NULL,
    PRIMARY KEY (from_id, to_id)
);

CREATE INDEX IF NOT EXISTS idx_entry_links_to ON entry_links(to_id);

CREATE TABLE IF NOT EXISTS search_docs (
    entry_id TEXT PRIMARY KEY NOT NULL,
    length   INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS search_postings (
    term     TEXT NOT NULL,
    entry_id TEXT NOT NULL,
    tf       INTEGER NOT NULL,
    PRIMARY KEY (term, entry_id)
);

CREATE INDEX IF NOT EXISTS idx_search_postings_entry ON search_postings(entry_id);

CREATE TABLE IF NOT EXISTS attachments (
    id          TEXT PRIMARY KEY NOT NULL,
    entry_id    TEXT NOT NULL,
    filename    TEXT NOT NULL,
    mime        TEXT NOT NULL,
    size        INTEGER NOT NULL,
    sha256      TEXT NOT NULL,
    storage_key TEXT NOT NULL,
    created_at  INTEGER NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_attachments_entry ON attachments(entry_id);

CREATE TABLE IF NOT EXISTS aliases (
    alias    TEXT PRIMARY KEY NOT NULL,
    entry_id TEXT NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_aliases_entry ON aliases(entry_id);

CREATE TABLE IF NOT EXISTS tags (
    tag      TEXT NOT NULL,
    entry_id TEXT NOT NULL,
    PRIMARY KEY (tag, entry_id)
);

CREATE INDEX IF NOT EXISTS idx_tags_entry ON tags(entry_id);

CREATE TABLE IF NOT EXISTS entry_versions (
    entry_id   TEXT NOT NULL,
    version    INTEGER NOT NULL,
    snapshot   TEXT NOT NULL,
    created_at INTEGER NOT NULL,
    PRIMARY KEY (entry_id, version)
);
";

/// Turso 存储后端。
pub struct TursoWikiStorage {
    conn: Mutex<Connection>,
    /// 附件 blob 后端;未配置时 attach/get 报错,list(纯元数据)仍可用。
    blob: Option<Arc<dyn BlobStore>>,
}

impl TursoWikiStorage {
    /// 打开(或创建)`path` 处的库并执行幂等 schema 初始化。
    ///
    /// 非 `:memory:` 路径:父目录自动创建,开多进程 WAL + 5 s busy
    /// timeout。若磁盘上的 WAL 边车处于撕裂状态(如 TUI 被 SIGKILL 在
    /// 事务中途),把 `-wal`/`-shm`/`-twal`/`-tshm` 隔离改名后重试一次;
    /// 主 `.db` 文件不动(kms 同款恢复逻辑)。
    pub async fn open(path: &str) -> Result<Self> {
        if path != ":memory:" {
            if let Some(parent) = Path::new(path).parent() {
                if !parent.as_os_str().is_empty() {
                    std::fs::create_dir_all(parent).ok();
                }
            }
        }

        match Self::try_open(path).await {
            Ok(storage) => Ok(storage),
            Err(error) if path != ":memory:" && is_torn_wal_error(&error.to_string()) => {
                tracing::warn!(
                    db = path,
                    error = %error,
                    "WAL torn on open — quarantining -wal/-shm sidecars and retrying once"
                );
                quarantine_wal_sidecars(Path::new(path));
                Self::try_open(path).await
            }
            Err(error) => Err(error),
        }
    }

    /// 内存库(测试用)。
    pub async fn open_in_memory() -> Result<Self> {
        Self::open(":memory:").await
    }

    async fn try_open(path: &str) -> Result<Self> {
        let mut builder = turso::Builder::new_local(path);
        if path != ":memory:" {
            builder = builder.experimental_multiprocess_wal(true);
        }
        let db = builder.build().await?;
        // schema 初始化在一次性连接上跑:初始化里的 pragma 探测是读语
        // 句,而 turso 连接自第一条读语句起钉住 MVCC 快照(REPEATABLE
        // READ)——在长驻连接上探测会把快照钉在打开时刻,让本实例永远
        // 看不到其他实例的后续提交。临时连接跑完即弃,不影响后续读。
        {
            let init_conn = db.connect()?;
            if path != ":memory:" {
                init_conn.pragma_update("busy_timeout", 5000).await?;
            }
            init_schema_on(&init_conn).await?;
        }
        let conn = db.connect()?;
        if path != ":memory:" {
            conn.pragma_update("busy_timeout", 5000).await?;
        }
        Ok(Self {
            conn: Mutex::new(conn),
            blob: None,
        })
    }

    /// 配置附件 blob 后端(生产用 `vfs_blob::VfsBlobStore`,测试用
    /// `blob::MemoryBlobStore`)。未配置时 attach/get 报错,list(纯
    /// 元数据)仍可用。
    pub fn with_blob_store(mut self, blob: Arc<dyn BlobStore>) -> Self {
        self.blob = Some(blob);
        self
    }

    /// 覆盖式重写 `from` 的全部出边(目标为词条 id)。
    ///
    /// 写路径正常情况下由 content 的 wiki 链接自动维护(见
    /// [`TursoWikiStorage`]),本方法供测试与工具显式覆写。单事务:旧边
    /// 删净、新边 `INSERT OR IGNORE` 入库,重复目标自动去重。
    pub async fn replace_out_links(&self, from: &str, targets: &[String]) -> Result<()> {
        let conn = self.conn.lock().await;
        run_in_txn(&conn, |conn| {
            Box::pin(async move {
                set_out_links(conn, from, targets).await?;
                Ok(())
            })
        })
        .await
    }
}

#[async_trait]
impl WikiEntryStorage for TursoWikiStorage {
    async fn create_entry(&self, entry: KnowledgeEntryData) -> Result<()> {
        let entry = normalize_entry(entry)?;
        let conn = self.conn.lock().await;
        run_in_txn(&conn, |conn| {
            Box::pin(async move {
                insert_entry(conn, &entry).await?;
                write_derived(conn, &entry).await?;
                Ok(())
            })
        })
        .await
    }

    async fn delete_entry(&self, id: uuid::Uuid) -> Result<()> {
        let id = id.to_string();
        let now = unix_now();
        let conn = self.conn.lock().await;
        run_in_txn(&conn, |conn| {
            Box::pin(async move {
                // 软删除:只盖 deleted_at;衍生行(倒排/链接/别名/版本)
                // 原样保留,restore 即恢复。已删则不动(幂等)。
                conn.execute(
                    "UPDATE entries SET deleted_at = ?2 \
                     WHERE id = ?1 AND deleted_at IS NULL",
                    turso::params![id, now],
                )
                .await?;
                Ok(())
            })
        })
        .await
    }

    async fn restore_entry(&self, id: uuid::Uuid) -> Result<()> {
        let id = id.to_string();
        let conn = self.conn.lock().await;
        let mut rows = conn
            .query(
                "SELECT deleted_at FROM entries WHERE id = ?1",
                turso::params![id.clone()],
            )
            .await?;
        match rows.next().await? {
            None => Err(Error::NotFound(id)),
            Some(row) => {
                if row.get_value(0)? == Value::Null {
                    Ok(()) // 未删除,幂等 no-op
                } else {
                    conn.execute(
                        "UPDATE entries SET deleted_at = NULL WHERE id = ?1",
                        turso::params![id],
                    )
                    .await?;
                    Ok(())
                }
            }
        }
    }

    async fn purge_entry(&self, id: uuid::Uuid) -> Result<()> {
        let id = id.to_string();
        let conn = self.conn.lock().await;
        // 事务内清行并收集 blob key;COMMIT 成功后再尽力删 blob ——
        // ROLLBACK 时 blob 仍在,不会出现行回滚而数据丢失。
        let mut blob_keys: Vec<String> = Vec::new();
        run_in_txn(&conn, |conn| {
            let keys = &mut blob_keys;
            Box::pin(async move {
                let mut rows = conn
                    .query(
                        "SELECT storage_key FROM attachments WHERE entry_id = ?1",
                        turso::params![id.clone()],
                    )
                    .await?;
                while let Some(row) = rows.next().await? {
                    keys.push(text_col(&row, 0)?);
                }
                conn.execute(
                    "DELETE FROM entry_links WHERE from_id = ?1 OR to_id = ?1",
                    turso::params![id.clone()],
                )
                .await?;
                conn.execute(
                    "DELETE FROM search_postings WHERE entry_id = ?1",
                    turso::params![id.clone()],
                )
                .await?;
                conn.execute(
                    "DELETE FROM search_docs WHERE entry_id = ?1",
                    turso::params![id.clone()],
                )
                .await?;
                conn.execute(
                    "DELETE FROM aliases WHERE entry_id = ?1",
                    turso::params![id.clone()],
                )
                .await?;
                conn.execute(
                    "DELETE FROM tags WHERE entry_id = ?1",
                    turso::params![id.clone()],
                )
                .await?;
                conn.execute(
                    "DELETE FROM entry_versions WHERE entry_id = ?1",
                    turso::params![id.clone()],
                )
                .await?;
                conn.execute(
                    "DELETE FROM attachments WHERE entry_id = ?1",
                    turso::params![id.clone()],
                )
                .await?;
                conn.execute("DELETE FROM entries WHERE id = ?1", turso::params![id])
                    .await?;
                Ok(())
            })
        })
        .await?;
        if let Some(blob) = &self.blob {
            for key in &blob_keys {
                if let Err(error) = blob.delete(key).await {
                    // 孤儿 blob(行已删、数据残留)只记日志:无害,可
                    // 由后端侧 GC 兜底。
                    tracing::warn!(key = %key, %error, "purge: failed to delete attachment blob");
                }
            }
        }
        Ok(())
    }

    async fn list_deleted(&self) -> Result<Vec<RecycleItem>> {
        let conn = self.conn.lock().await;
        let mut rows = conn
            .query(
                "SELECT id, title, alias, content, ctime, mtime, deleted_at FROM entries \
                 WHERE deleted_at IS NOT NULL ORDER BY deleted_at, id",
                (),
            )
            .await?;
        let mut items = Vec::new();
        while let Some(row) = rows.next().await? {
            let deleted_at = int_col(&row, 6)?;
            let mut entry = entry_from_row(&row)?;
            entry.tags = fetch_tags(&conn, &entry.id).await?;
            items.push(RecycleItem { entry, deleted_at });
        }
        Ok(items)
    }

    async fn get_entry(&self, id: uuid::Uuid) -> Result<KnowledgeEntryData> {
        let conn = self.conn.lock().await;
        let mut rows = conn
            .query(
                "SELECT id, title, alias, content, ctime, mtime FROM entries \
                 WHERE id = ?1 AND deleted_at IS NULL",
                turso::params![id.to_string()],
            )
            .await?;
        match rows.next().await? {
            Some(row) => {
                let mut entry = entry_from_row(&row)?;
                entry.tags = fetch_tags(&conn, &entry.id).await?;
                Ok(entry)
            }
            None => Err(Error::NotFound(id.to_string())),
        }
    }

    async fn list_entries(&self, filter: &ListFilter) -> Result<Vec<ListEntrySummary>> {
        if filter.limit == 0 {
            return Ok(Vec::new());
        }
        // 规范化标签:trim、去空、去重(与写入口径一致)。
        let wanted: Vec<String> = filter
            .tags
            .iter()
            .map(|t| t.trim().to_string())
            .filter(|t| !t.is_empty())
            .collect::<HashSet<_>>()
            .into_iter()
            .collect();

        let conn = self.conn.lock().await;
        let mut sql = String::from(
            "SELECT e.id, e.title, e.alias, e.ctime, e.mtime FROM entries e \
             WHERE e.deleted_at IS NULL",
        );
        let mut params: Vec<Value> = Vec::new();
        if let Some(since) = filter.mtime_since {
            params.push(Value::Integer(since));
            sql.push_str(&format!(" AND e.mtime >= ?{}", params.len()));
        }
        if let Some(until) = filter.mtime_until {
            params.push(Value::Integer(until));
            sql.push_str(&format!(" AND e.mtime <= ?{}", params.len()));
        }
        if !wanted.is_empty() {
            // AND 语义:词条须含全部标签 → 子查询按词条数标签去重计数。
            let placeholders = (1..=wanted.len())
                .map(|position| format!("?{}", params.len() + position))
                .collect::<Vec<_>>()
                .join(", ");
            params.extend(wanted.iter().map(text_value));
            params.push(Value::Integer(wanted.len() as i64));
            sql.push_str(&format!(
                " AND e.id IN (SELECT entry_id FROM tags WHERE tag IN ({placeholders}) \
                 GROUP BY entry_id HAVING COUNT(DISTINCT tag) = ?{})",
                params.len()
            ));
        }
        // mtime 降序 + id 升序:同 mtime 时输出稳定,分页不重不漏。
        params.push(Value::Integer(filter.limit as i64));
        sql.push_str(&format!(
            " ORDER BY e.mtime DESC, e.id LIMIT ?{}",
            params.len()
        ));
        if filter.offset > 0 {
            params.push(Value::Integer(filter.offset as i64));
            sql.push_str(&format!(" OFFSET ?{}", params.len()));
        }

        let mut rows = conn.query(&sql, turso::params_from_iter(params)).await?;
        let mut out = Vec::new();
        while let Some(row) = rows.next().await? {
            let id = text_col(&row, 0)?;
            let alias: Vec<String> = serde_json::from_str(&text_col(&row, 2)?)?;
            out.push(ListEntrySummary {
                tags: fetch_tags(&conn, &id).await?,
                id,
                title: text_col(&row, 1)?,
                alias,
                ctime: int_col(&row, 3)?,
                mtime: int_col(&row, 4)?,
            });
        }
        Ok(out)
    }

    async fn find_by_title(&self, title: &str) -> Result<Option<KnowledgeEntryData>> {
        let conn = self.conn.lock().await;
        let mut rows = conn
            .query(
                "SELECT id, title, alias, content, ctime, mtime FROM entries \
                 WHERE title = ?1 AND deleted_at IS NULL",
                turso::params![title.to_string()],
            )
            .await?;
        match rows.next().await? {
            Some(row) => {
                let mut entry = entry_from_row(&row)?;
                entry.tags = fetch_tags(&conn, &entry.id).await?;
                Ok(Some(entry))
            }
            None => Ok(None),
        }
    }

    async fn get_entry_batch(&self, ids: Vec<uuid::Uuid>) -> Result<Vec<KnowledgeEntryData>> {
        if ids.is_empty() {
            return Ok(Vec::new());
        }
        let conn = self.conn.lock().await;
        let mut by_id: HashMap<String, KnowledgeEntryData> = HashMap::new();
        for chunk in ids.chunks(IN_CHUNK) {
            let placeholders = (1..=chunk.len())
                .map(|position| format!("?{position}"))
                .collect::<Vec<_>>()
                .join(", ");
            let sql = format!(
                "SELECT id, title, alias, content, ctime, mtime FROM entries \
                 WHERE id IN ({placeholders}) AND deleted_at IS NULL"
            );
            let mut rows = conn
                .query(&sql, turso::params_from_iter(chunk.iter().map(uuid_value)))
                .await?;
            while let Some(row) = rows.next().await? {
                let entry = entry_from_row(&row)?;
                by_id.insert(entry.id.clone(), entry);
            }
        }
        let tag_map = fetch_tags_batch(&conn, by_id.keys()).await?;
        for entry in by_id.values_mut() {
            entry.tags = tag_map.get(&entry.id).cloned().unwrap_or_default();
        }
        // 按入参顺序输出,重复 id 只取一次,未命中的跳过。
        let mut seen = HashSet::new();
        let mut out = Vec::new();
        for id in &ids {
            if seen.insert(*id) {
                if let Some(entry) = by_id.get(&id.to_string()) {
                    out.push(entry.clone());
                }
            }
        }
        Ok(out)
    }

    async fn update_entry(&self, entry: KnowledgeEntryData) -> Result<()> {
        let entry = normalize_entry(entry)?;
        let conn = self.conn.lock().await;
        run_in_txn(&conn, |conn| {
            Box::pin(async move {
                update_entry_row(conn, &entry).await?;
                write_derived(conn, &entry).await?;
                Ok(())
            })
        })
        .await
    }

    async fn update_entry_batch(&self, entries: &[KnowledgeEntryData]) -> Result<()> {
        if entries.is_empty() {
            return Ok(());
        }
        // 规范化全部通过后再开事务:空标题在写任何东西之前就被拒绝。
        let entries: Vec<KnowledgeEntryData> = entries
            .iter()
            .cloned()
            .map(normalize_entry)
            .collect::<Result<_>>()?;
        let conn = self.conn.lock().await;
        run_in_txn(&conn, |conn| {
            Box::pin(async move {
                for entry in &entries {
                    update_entry_row(conn, entry).await?;
                    write_derived(conn, entry).await?;
                }
                Ok(())
            })
        })
        .await
    }

    async fn list_versions(&self, id: uuid::Uuid) -> Result<Vec<EntryVersionSummary>> {
        let conn = self.conn.lock().await;
        let mut rows = conn
            .query(
                "SELECT version, created_at FROM entry_versions WHERE entry_id = ?1 \
                 ORDER BY version DESC",
                turso::params![id.to_string()],
            )
            .await?;
        let mut out = Vec::new();
        while let Some(row) = rows.next().await? {
            out.push(EntryVersionSummary {
                version: int_col(&row, 0)?,
                created_at: int_col(&row, 1)?,
            });
        }
        Ok(out)
    }

    async fn get_version(&self, id: uuid::Uuid, version: i64) -> Result<KnowledgeEntryData> {
        let conn = self.conn.lock().await;
        let mut rows = conn
            .query(
                "SELECT snapshot FROM entry_versions WHERE entry_id = ?1 AND version = ?2",
                turso::params![id.to_string(), version],
            )
            .await?;
        match rows.next().await? {
            Some(row) => Ok(serde_json::from_str(&text_col(&row, 0)?)?),
            None => Err(Error::NotFound(format!("{} v{version}", id))),
        }
    }
}

#[async_trait]
impl WikiIndexStorage for TursoWikiStorage {
    async fn get_in_links_batch(&self, ids: &[String]) -> Result<Vec<Vec<String>>> {
        fetch_adjacency(&self.conn, ids, "to_id", "from_id").await
    }

    async fn get_out_links_batch(&self, ids: &[String]) -> Result<Vec<Vec<String>>> {
        fetch_adjacency(&self.conn, ids, "from_id", "to_id").await
    }

    async fn get_mentions(&self, id: &str) -> Result<Vec<String>> {
        let conn = self.conn.lock().await;
        // 目标必须存在且未删除;标题 + 全部别名都是提及探测词。
        let mut rows = conn
            .query(
                "SELECT title, alias FROM entries WHERE id = ?1 AND deleted_at IS NULL",
                turso::params![id.to_string()],
            )
            .await?;
        let Some(row) = rows.next().await? else {
            return Err(Error::NotFound(id.to_string()));
        };
        let title = text_col(&row, 0)?;
        let alias: Vec<String> = serde_json::from_str(&text_col(&row, 1)?)?;
        drop(rows);

        let mut terms: Vec<String> = Vec::new();
        for term in std::iter::once(title).chain(alias) {
            let term = term.trim().to_string();
            if !term.is_empty() && !terms.contains(&term) {
                terms.push(term);
            }
        }

        let mut mentions: Vec<String> = Vec::new();
        for term in &terms {
            // 未链接的纯文本提及:正文含该词,且没有 [[链接]] 指向目标。
            let pattern = format!("%{}%", like_escape(term));
            let mut rows = conn
                .query(
                    "SELECT DISTINCT e.id FROM entries e \
                     WHERE e.deleted_at IS NULL AND e.id != ?1 \
                     AND e.id NOT IN (SELECT from_id FROM entry_links WHERE to_id = ?1) \
                     AND e.content LIKE ?2 ESCAPE '\\' ORDER BY e.id",
                    turso::params![id.to_string(), pattern],
                )
                .await?;
            while let Some(row) = rows.next().await? {
                let hit = text_col(&row, 0)?;
                if !mentions.contains(&hit) {
                    mentions.push(hit);
                }
            }
        }
        mentions.sort();
        Ok(mentions)
    }
}

#[async_trait]
impl WikiSearchStorage for TursoWikiStorage {
    async fn search_with_filter(
        &self,
        query: &str,
        limit: usize,
        filter: &SearchFilter,
    ) -> Result<Vec<SearchHit>> {
        if limit == 0 {
            return Ok(Vec::new());
        }
        // 查询 token 去重(保序):重复词不应重复计分。
        let mut terms: Vec<String> = Vec::new();
        for token in tokenize(query) {
            if !terms.contains(&token) {
                terms.push(token);
            }
        }
        if terms.is_empty() {
            return Ok(Vec::new());
        }
        let conn = self.conn.lock().await;

        // 全库统计只算未删除词条(N 与平均文档长度)。
        let mut stats_rows = conn
            .query(
                "SELECT COUNT(*), COALESCE(AVG(d.length), 0.0) \
                 FROM search_docs d JOIN entries e ON e.id = d.entry_id \
                 WHERE e.deleted_at IS NULL",
                (),
            )
            .await?;
        let (n_docs, avgdl) = match stats_rows.next().await? {
            Some(row) => (int_col(&row, 0)? as usize, real_col(&row, 1)?),
            None => return Ok(Vec::new()),
        };
        if n_docs == 0 {
            return Ok(Vec::new());
        }

        // 候选 postings(查询词的全部命中,只算未删除词条)。
        let mut tf: HashMap<String, HashMap<String, i64>> = HashMap::new(); // entry -> term -> tf
        let mut df: HashMap<String, usize> = HashMap::new(); // term -> 含该词文档数
        for chunk in terms.chunks(IN_CHUNK) {
            let placeholders = (1..=chunk.len())
                .map(|position| format!("?{position}"))
                .collect::<Vec<_>>()
                .join(", ");
            let sql = format!(
                "SELECT p.term, p.entry_id, p.tf FROM search_postings p \
                 JOIN entries e ON e.id = p.entry_id \
                 WHERE p.term IN ({placeholders}) AND e.deleted_at IS NULL"
            );
            let mut rows = conn
                .query(&sql, turso::params_from_iter(chunk.iter().map(text_value)))
                .await?;
            while let Some(row) = rows.next().await? {
                let term = text_col(&row, 0)?;
                let entry_id = text_col(&row, 1)?;
                let count = int_col(&row, 2)?;
                tf.entry(entry_id).or_default().insert(term.clone(), count);
                *df.entry(term).or_insert(0) += 1;
            }
        }
        if tf.is_empty() {
            return Ok(Vec::new());
        }

        // 候选元数据:文档长度 + mtime(时间窗在此过滤)。
        let candidates: Vec<String> = tf.keys().cloned().collect();
        let mut meta: HashMap<String, (i64, i64)> = HashMap::new(); // id -> (mtime, length)
        for chunk in candidates.chunks(IN_CHUNK) {
            let placeholders = (1..=chunk.len())
                .map(|position| format!("?{position}"))
                .collect::<Vec<_>>()
                .join(", ");
            let sql = format!(
                "SELECT e.id, e.mtime, d.length FROM entries e \
                 JOIN search_docs d ON d.entry_id = e.id \
                 WHERE e.id IN ({placeholders}) AND e.deleted_at IS NULL"
            );
            let mut rows = conn
                .query(&sql, turso::params_from_iter(chunk.iter().map(text_value)))
                .await?;
            while let Some(row) = rows.next().await? {
                let entry_id = text_col(&row, 0)?;
                let mtime = int_col(&row, 1)?;
                let length = int_col(&row, 2)?;
                let since_ok = filter.mtime_since.is_none_or(|s| mtime >= s);
                let until_ok = filter.mtime_until.is_none_or(|u| mtime <= u);
                if since_ok && until_ok {
                    meta.insert(entry_id, (mtime, length));
                }
            }
        }

        // 标签 AND 过滤:每个候选须含全部过滤标签。
        if !filter.tags.is_empty() {
            let wanted: Vec<String> = filter
                .tags
                .iter()
                .map(|t| t.trim().to_string())
                .filter(|t| !t.is_empty())
                .collect::<HashSet<_>>()
                .into_iter()
                .collect();
            if wanted.is_empty() {
                return Ok(Vec::new());
            }
            let mut tag_hits: HashMap<String, usize> = HashMap::new();
            let live: Vec<String> = meta.keys().cloned().collect();
            for chunk in live.chunks(IN_CHUNK) {
                let entry_placeholders = (1..=chunk.len())
                    .map(|position| format!("?{position}"))
                    .collect::<Vec<_>>()
                    .join(", ");
                let tag_placeholders = (chunk.len() + 1..=chunk.len() + wanted.len())
                    .map(|position| format!("?{position}"))
                    .collect::<Vec<_>>()
                    .join(", ");
                let sql = format!(
                    "SELECT entry_id, tag FROM tags \
                     WHERE entry_id IN ({entry_placeholders}) AND tag IN ({tag_placeholders})"
                );
                let mut params: Vec<Value> = chunk.iter().map(text_value).collect();
                params.extend(wanted.iter().map(text_value));
                let mut rows = conn.query(&sql, turso::params_from_iter(params)).await?;
                while let Some(row) = rows.next().await? {
                    let entry_id = text_col(&row, 0)?;
                    // 同一 (tag, entry) 至多一行,计数即可判「全含」。
                    *tag_hits.entry(entry_id).or_insert(0) += 1;
                }
            }
            meta.retain(|id, _| tag_hits.get(id).is_some_and(|&n| n >= wanted.len()));
        }

        // BM25 打分:score(d) = Σ_t idf(t) · tf 部分。
        let mut hits: Vec<SearchHit> = tf
            .into_iter()
            .filter_map(|(entry_id, term_tf)| {
                let &(_mtime, dl) = meta.get(&entry_id)?;
                let mut score = 0.0_f64;
                for term in &terms {
                    let Some(&count) = term_tf.get(term) else {
                        continue;
                    };
                    let df = df.get(term).copied().unwrap_or(0);
                    score += idf(n_docs, df) * bm25_term(count, dl, avgdl);
                }
                (score > 0.0).then_some(SearchHit { entry_id, score })
            })
            .collect();
        // 相关性降序,同分按 entry_id 升序,保证确定性。
        hits.sort_by(|a, b| {
            b.score
                .total_cmp(&a.score)
                .then_with(|| a.entry_id.cmp(&b.entry_id))
        });
        hits.truncate(limit);
        Ok(hits)
    }
}

/// 幂等 schema 初始化:建表 + 旧库补列(`CREATE TABLE IF NOT EXISTS`
/// 不会给已存在的表加列,kms 的 pragma 探测同款)。须在一次性连接上
/// 执行,理由见 [`TursoWikiStorage::open`]。
async fn init_schema_on(conn: &Connection) -> Result<()> {
    conn.execute_batch(SCHEMA_SQL).await?;
    let mut columns = conn
        .query(
            "SELECT name FROM pragma_table_info('entries') WHERE name = 'deleted_at'",
            (),
        )
        .await?;
    let has_deleted_at = columns.next().await?.is_some();
    drop(columns);
    if !has_deleted_at {
        conn.execute_batch("ALTER TABLE entries ADD COLUMN deleted_at INTEGER;")
            .await?;
    }
    Ok(())
}

#[async_trait]
impl WikiAttachmentStorage for TursoWikiStorage {
    async fn attach_image(
        &self,
        entry_id: uuid::Uuid,
        filename: String,
        data: Vec<u8>,
    ) -> Result<AttachmentMeta> {
        let Some((mime, ext)) = image_type_from_filename(&filename) else {
            return Err(Error::Unknown(format!(
                "不支持的图像格式:{filename}(白名单:png/jpg/jpeg/gif/webp/svg/bmp/avif)"
            )));
        };
        if data.is_empty() {
            return Err(Error::Unknown("附件内容为空".to_string()));
        }
        let blob = self
            .blob
            .as_ref()
            .ok_or_else(|| Error::Unknown("未配置附件存储(with_blob_store)".to_string()))?;
        let entry_id_str = entry_id.to_string();
        let now = unix_now();
        let sha_hex: String = Sha256::digest(&data)
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();

        // 词条须存在且未删除。
        {
            let conn = self.conn.lock().await;
            let mut rows = conn
                .query(
                    "SELECT id FROM entries WHERE id = ?1 AND deleted_at IS NULL",
                    turso::params![entry_id_str.clone()],
                )
                .await?;
            if rows.next().await?.is_none() {
                return Err(Error::NotFound(entry_id_str));
            }
        }

        // key 按附件 id 一对一(不做内容寻址:免引用计数,purge/detach
        // 直接删 key;跨词条去重交给对象层 GC,取舍见模块文档)。
        let attachment_id = uuid::Uuid::new_v4();
        let key = format!("attachments/{entry_id_str}/{attachment_id}.{ext}");
        blob.put(&key, data.clone()).await?;

        let meta = AttachmentMeta {
            id: attachment_id.to_string(),
            entry_id: entry_id_str,
            filename,
            mime: mime.to_string(),
            size: data.len() as i64,
            sha256: sha_hex,
            created_at: now,
        };
        let insert = async {
            let conn = self.conn.lock().await;
            conn.execute(
                "INSERT INTO attachments (id, entry_id, filename, mime, size, sha256, storage_key, created_at) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                turso::params![
                    meta.id.clone(),
                    meta.entry_id.clone(),
                    meta.filename.clone(),
                    meta.mime.clone(),
                    meta.size,
                    meta.sha256.clone(),
                    key.clone(),
                    meta.created_at,
                ],
            )
            .await
        };
        if let Err(error) = insert.await {
            // 行失败 → 回滚 blob,不留孤儿。
            let _ = blob.delete(&key).await;
            return Err(Error::from(error));
        }
        Ok(meta)
    }

    async fn list_attachments(&self, entry_id: uuid::Uuid) -> Result<Vec<AttachmentMeta>> {
        let conn = self.conn.lock().await;
        let mut rows = conn
            .query(
                "SELECT id, entry_id, filename, mime, size, sha256, created_at \
                 FROM attachments WHERE entry_id = ?1 ORDER BY created_at, id",
                turso::params![entry_id.to_string()],
            )
            .await?;
        let mut out = Vec::new();
        while let Some(row) = rows.next().await? {
            out.push(attachment_from_row(&row)?);
        }
        Ok(out)
    }

    async fn get_attachment(
        &self,
        entry_id: uuid::Uuid,
        attachment_id: uuid::Uuid,
    ) -> Result<(AttachmentMeta, Vec<u8>)> {
        let (meta, key) = {
            let conn = self.conn.lock().await;
            let mut rows = conn
                .query(
                    "SELECT id, entry_id, filename, mime, size, sha256, created_at, storage_key \
                     FROM attachments WHERE id = ?1 AND entry_id = ?2",
                    turso::params![attachment_id.to_string(), entry_id.to_string()],
                )
                .await?;
            match rows.next().await? {
                Some(row) => {
                    let key = text_col(&row, 7)?;
                    (attachment_from_row(&row)?, key)
                }
                None => return Err(Error::NotFound(attachment_id.to_string())),
            }
        };
        let blob = self
            .blob
            .as_ref()
            .ok_or_else(|| Error::Unknown("未配置附件存储(with_blob_store)".to_string()))?;
        let data = blob.get(&key).await?;
        Ok((meta, data))
    }

    async fn detach_attachment(
        &self,
        entry_id: uuid::Uuid,
        attachment_id: uuid::Uuid,
    ) -> Result<()> {
        let key: Option<String> = {
            let conn = self.conn.lock().await;
            let mut rows = conn
                .query(
                    "SELECT storage_key FROM attachments WHERE id = ?1 AND entry_id = ?2",
                    turso::params![attachment_id.to_string(), entry_id.to_string()],
                )
                .await?;
            let key = match rows.next().await? {
                Some(row) => Some(text_col(&row, 0)?),
                None => None,
            };
            if key.is_some() {
                conn.execute(
                    "DELETE FROM attachments WHERE id = ?1 AND entry_id = ?2",
                    turso::params![attachment_id.to_string(), entry_id.to_string()],
                )
                .await?;
            }
            key
        };
        if let Some(key) = key {
            if let Some(blob) = &self.blob {
                if let Err(error) = blob.delete(&key).await {
                    tracing::warn!(key = %key, %error, "detach: failed to delete attachment blob");
                }
            }
        }
        // 行不存在 → 幂等 no-op(契约)。
        Ok(())
    }
}

/// 扩展名 → (MIME, 规范化小写扩展名);仅图像白名单。
fn image_type_from_filename(name: &str) -> Option<(String, String)> {
    let dot = name.rfind('.')?;
    let ext = name[dot + 1..].to_ascii_lowercase();
    let mime = match ext.as_str() {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "svg" => "image/svg+xml",
        "bmp" => "image/bmp",
        "avif" => "image/avif",
        _ => return None,
    };
    Some((mime.to_string(), ext))
}

fn attachment_from_row(row: &turso::Row) -> Result<AttachmentMeta> {
    Ok(AttachmentMeta {
        id: text_col(row, 0)?,
        entry_id: text_col(row, 1)?,
        filename: text_col(row, 2)?,
        mime: text_col(row, 3)?,
        size: int_col(row, 4)?,
        sha256: text_col(row, 5)?,
        created_at: int_col(row, 6)?,
    })
}

// ─────────────────────── 词条行读写(自由函数,单/批量共用) ───────────────────────

/// 写入前规范化与校验:trim 首尾空白;全空白标题拒绝。
/// 不折叠大小写——`BRCA1` 与 `brca1` 对专业缩写是不同词条,保持区分。
fn normalize_entry(entry: KnowledgeEntryData) -> Result<KnowledgeEntryData> {
    let title = entry.title.trim().to_string();
    if title.is_empty() {
        return Err(Error::EmptyTitle);
    }
    Ok(KnowledgeEntryData { title, ..entry })
}

/// 事务内预检标题占用(trim 后精确匹配,含回收站中未净删的词条):
/// 命中返回占用者 id;`exclude_id` 跳过自身(重命名为现名不算占用)。
/// 预检只求友好错误,并发窗口仍由 `UNIQUE(title)` 约束兜底。
async fn title_owner(
    conn: &Connection,
    title: &str,
    exclude_id: Option<&str>,
) -> Result<Option<String>> {
    let mut rows = match exclude_id {
        Some(id) => {
            conn.query(
                "SELECT id FROM entries WHERE title = ?1 AND id != ?2",
                turso::params![title.to_string(), id.to_string()],
            )
            .await?
        }
        None => {
            conn.query(
                "SELECT id FROM entries WHERE title = ?1",
                turso::params![title.to_string()],
            )
            .await?
        }
    };
    match rows.next().await? {
        Some(row) => Ok(Some(text_col(&row, 0)?)),
        None => Ok(None),
    }
}

async fn insert_entry(conn: &Connection, entry: &KnowledgeEntryData) -> Result<()> {
    if let Some(existing_id) = title_owner(conn, &entry.title, None).await? {
        return Err(Error::DuplicateTitle {
            title: entry.title.clone(),
            existing_id,
        });
    }
    conn.execute(
        "INSERT INTO entries (id, title, alias, content, ctime, mtime) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        turso::params![
            entry.id.clone(),
            entry.title.clone(),
            serde_json::to_string(&entry.alias)?,
            entry.content.clone(),
            entry.ctime,
            entry.mtime,
        ],
    )
    .await?;
    Ok(())
}

async fn update_entry_row(conn: &Connection, entry: &KnowledgeEntryData) -> Result<()> {
    if let Some(existing_id) = title_owner(conn, &entry.title, Some(&entry.id)).await? {
        return Err(Error::DuplicateTitle {
            title: entry.title.clone(),
            existing_id,
        });
    }
    let affected = conn
        .execute(
            "UPDATE entries SET title = ?2, alias = ?3, content = ?4, ctime = ?5, mtime = ?6 \
             WHERE id = ?1 AND deleted_at IS NULL",
            turso::params![
                entry.id.clone(),
                entry.title.clone(),
                serde_json::to_string(&entry.alias)?,
                entry.content.clone(),
                entry.ctime,
                entry.mtime,
            ],
        )
        .await?;
    if affected == 0 {
        return Err(Error::NotFound(entry.id.clone()));
    }
    Ok(())
}

/// 词条行写入后的全部衍生数据维护:倒排、别名/标签、出边、版本快照。
/// 须在调用方事务内执行。
async fn write_derived(conn: &Connection, entry: &KnowledgeEntryData) -> Result<()> {
    reindex_entry(conn, entry).await?;
    rebuild_out_links(conn, entry).await?;
    snapshot_version(conn, entry).await?;
    Ok(())
}

/// 重建 `entry` 的检索倒排与别名/标签索引:文档 = title + alias +
/// content 拼接后分词。幂等:先清该条目旧行再写。
async fn reindex_entry(conn: &Connection, entry: &KnowledgeEntryData) -> Result<()> {
    for table in ["search_postings", "search_docs", "aliases", "tags"] {
        conn.execute(
            &format!("DELETE FROM {table} WHERE entry_id = ?1"),
            turso::params![entry.id.clone()],
        )
        .await?;
    }

    let mut doc = String::with_capacity(entry.title.len() + entry.content.len() + 16);
    doc.push_str(&entry.title);
    for alias in &entry.alias {
        doc.push('\n');
        doc.push_str(alias);
    }
    doc.push('\n');
    doc.push_str(&entry.content);
    let tokens = tokenize(&doc);

    conn.execute(
        "INSERT INTO search_docs (entry_id, length) VALUES (?1, ?2)",
        turso::params![entry.id.clone(), tokens.len() as i64],
    )
    .await?;

    let mut term_tf: HashMap<&str, i64> = HashMap::new();
    for token in &tokens {
        *term_tf.entry(token.as_str()).or_insert(0) += 1;
    }
    for (term, count) in term_tf {
        conn.execute(
            "INSERT INTO search_postings (term, entry_id, tf) VALUES (?1, ?2, ?3)",
            turso::params![term, entry.id.clone(), count],
        )
        .await?;
    }

    // 别名:trim、去空、去重;INSERT OR IGNORE —— 撞其他词条的别名时
    // 先占者赢,解析口径稳定(先标题后别名,见 resolve_target)。
    let mut aliases: HashSet<&str> = HashSet::new();
    for alias in &entry.alias {
        let alias = alias.trim();
        if !alias.is_empty() {
            aliases.insert(alias);
        }
    }
    for alias in aliases {
        conn.execute(
            "INSERT OR IGNORE INTO aliases (alias, entry_id) VALUES (?1, ?2)",
            turso::params![alias, entry.id.clone()],
        )
        .await?;
    }

    // 标签:同样 trim、去空、去重;大小写不折叠(与标题口径一致)。
    let mut tags: HashSet<&str> = HashSet::new();
    for tag in &entry.tags {
        let tag = tag.trim();
        if !tag.is_empty() {
            tags.insert(tag);
        }
    }
    for tag in tags {
        conn.execute(
            "INSERT OR IGNORE INTO tags (tag, entry_id) VALUES (?1, ?2)",
            turso::params![tag, entry.id.clone()],
        )
        .await?;
    }
    Ok(())
}

/// 从 `entry.content` 提取 wiki 链接并解析目标(先标题后别名,均要求
/// 未删除;悬空目标跳过,自指边跳过),覆盖式重建出边。
async fn rebuild_out_links(conn: &Connection, entry: &KnowledgeEntryData) -> Result<()> {
    let mut targets: Vec<String> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();
    for link in Parser::new().extract_links(&entry.content) {
        if link.kind != LinkKind::Wiki {
            continue;
        }
        if !seen.insert(link.target.clone()) {
            continue;
        }
        if let Some(id) = resolve_target(conn, &link.target).await? {
            if id != entry.id && !targets.contains(&id) {
                targets.push(id);
            }
        }
    }
    set_out_links(conn, &entry.id, &targets).await
}

/// 内链目标解析:先按标题(未删除),再按别名(未删除)。
async fn resolve_target(conn: &Connection, target: &str) -> Result<Option<String>> {
    let mut rows = conn
        .query(
            "SELECT id FROM entries WHERE title = ?1 AND deleted_at IS NULL",
            turso::params![target.to_string()],
        )
        .await?;
    if let Some(row) = rows.next().await? {
        return Ok(Some(text_col(&row, 0)?));
    }
    drop(rows);
    let mut rows = conn
        .query(
            "SELECT a.entry_id FROM aliases a JOIN entries e ON e.id = a.entry_id \
             WHERE a.alias = ?1 AND e.deleted_at IS NULL",
            turso::params![target.to_string()],
        )
        .await?;
    match rows.next().await? {
        Some(row) => Ok(Some(text_col(&row, 0)?)),
        None => Ok(None),
    }
}

/// 覆盖式写出边:删净旧边再插入;重复目标由调用方去重,这里 IGNORE 兜底。
async fn set_out_links(conn: &Connection, from: &str, targets: &[String]) -> Result<()> {
    conn.execute(
        "DELETE FROM entry_links WHERE from_id = ?1",
        turso::params![from],
    )
    .await?;
    for to in targets {
        conn.execute(
            "INSERT OR IGNORE INTO entry_links (from_id, to_id) VALUES (?1, ?2)",
            turso::params![from, to.as_str()],
        )
        .await?;
    }
    Ok(())
}

/// 追加一条全量版本快照(编号 = 现有最大版本 + 1;created_at 取词条
/// mtime,写入时刻由调用方语义决定)。writing-base 同款模式。
async fn snapshot_version(conn: &Connection, entry: &KnowledgeEntryData) -> Result<()> {
    let mut rows = conn
        .query(
            "SELECT COALESCE(MAX(version), 0) FROM entry_versions WHERE entry_id = ?1",
            turso::params![entry.id.clone()],
        )
        .await?;
    let next = match rows.next().await? {
        Some(row) => int_col(&row, 0)? + 1,
        None => 1,
    };
    conn.execute(
        "INSERT INTO entry_versions (entry_id, version, snapshot, created_at) \
         VALUES (?1, ?2, ?3, ?4)",
        turso::params![
            entry.id.clone(),
            next,
            serde_json::to_string(entry)?,
            entry.mtime,
        ],
    )
    .await?;
    Ok(())
}

/// 单词条标签(字典序)。
async fn fetch_tags(conn: &Connection, id: &str) -> Result<Vec<String>> {
    let mut rows = conn
        .query(
            "SELECT tag FROM tags WHERE entry_id = ?1 ORDER BY tag",
            turso::params![id.to_string()],
        )
        .await?;
    let mut out = Vec::new();
    while let Some(row) = rows.next().await? {
        out.push(text_col(&row, 0)?);
    }
    Ok(out)
}

/// 批量标签:id -> 标签列表(字典序)。
async fn fetch_tags_batch<'a>(
    conn: &Connection,
    ids: impl Iterator<Item = &'a String>,
) -> Result<HashMap<String, Vec<String>>> {
    let ids: Vec<String> = ids.cloned().collect();
    let mut map: HashMap<String, Vec<String>> = HashMap::new();
    for chunk in ids.chunks(IN_CHUNK) {
        let placeholders = (1..=chunk.len())
            .map(|position| format!("?{position}"))
            .collect::<Vec<_>>()
            .join(", ");
        let sql = format!("SELECT entry_id, tag FROM tags WHERE entry_id IN ({placeholders})");
        let mut rows = conn
            .query(&sql, turso::params_from_iter(chunk.iter().map(text_value)))
            .await?;
        while let Some(row) = rows.next().await? {
            map.entry(text_col(&row, 0)?)
                .or_default()
                .push(text_col(&row, 1)?);
        }
    }
    for tags in map.values_mut() {
        tags.sort();
    }
    Ok(map)
}

/// 邻接表批量查询:`WHERE {key} IN (...)`,把 `{value}` 按 key 分桶;
/// 邻居须是未删除词条。桶内按 value 排序保证确定性;`key`/`value`
/// 是编译期列名,非用户输入。
async fn fetch_adjacency(
    conn: &Mutex<Connection>,
    ids: &[String],
    key: &str,
    value: &str,
) -> Result<Vec<Vec<String>>> {
    if ids.is_empty() {
        return Ok(Vec::new());
    }
    let conn = conn.lock().await;
    let mut buckets: HashMap<String, Vec<String>> = HashMap::new();
    for chunk in ids.chunks(IN_CHUNK) {
        let placeholders = (1..=chunk.len())
            .map(|position| format!("?{position}"))
            .collect::<Vec<_>>()
            .join(", ");
        let sql = format!(
            "SELECT l.{key}, l.{value} FROM entry_links l \
             JOIN entries e ON e.id = l.{value} \
             WHERE l.{key} IN ({placeholders}) AND e.deleted_at IS NULL \
             ORDER BY l.{value}"
        );
        let mut rows = conn
            .query(&sql, turso::params_from_iter(chunk.iter().map(text_value)))
            .await?;
        while let Some(row) = rows.next().await? {
            let owner = text_col(&row, 0)?;
            let neighbor = text_col(&row, 1)?;
            buckets.entry(owner).or_default().push(neighbor);
        }
    }
    // get+clone 而非 remove:入参重复 id 时每个位置都要拿到同一桶。
    Ok(ids
        .iter()
        .map(|id| buckets.get(id).cloned().unwrap_or_default())
        .collect())
}

/// LIKE 模式转义:`\`、`%`、`_` 前加反斜杠(SQL 里配 `ESCAPE '\'`)。
fn like_escape(term: &str) -> String {
    let mut out = String::with_capacity(term.len());
    for ch in term.chars() {
        if matches!(ch, '\\' | '%' | '_') {
            out.push('\\');
        }
        out.push(ch);
    }
    out
}

fn unix_now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

// ─────────────────────── 行解析(手写;turso 无 FromRow) ───────────────────────

fn text_col(row: &turso::Row, idx: usize) -> Result<String> {
    match row.get_value(idx)? {
        Value::Text(s) => Ok(s),
        other => Err(Error::CorruptRow(format!(
            "expected TEXT at column {idx}, got {other:?}"
        ))),
    }
}

fn int_col(row: &turso::Row, idx: usize) -> Result<i64> {
    match row.get_value(idx)? {
        Value::Integer(v) => Ok(v),
        other => Err(Error::CorruptRow(format!(
            "expected INTEGER at column {idx}, got {other:?}"
        ))),
    }
}

fn real_col(row: &turso::Row, idx: usize) -> Result<f64> {
    match row.get_value(idx)? {
        Value::Real(v) => Ok(v),
        other => Err(Error::CorruptRow(format!(
            "expected REAL at column {idx}, got {other:?}"
        ))),
    }
}

fn entry_from_row(row: &turso::Row) -> Result<KnowledgeEntryData> {
    Ok(KnowledgeEntryData {
        alias: serde_json::from_str(&text_col(row, 2)?)?,
        id: text_col(row, 0)?,
        title: text_col(row, 1)?,
        tags: Vec::new(), // 标签在独立表,由调用方 fetch_tags(_batch) 填充
        content: text_col(row, 3)?,
        ctime: int_col(row, 4)?,
        mtime: int_col(row, 5)?,
    })
}

// ─────────────────────── 事务(kms 同款显式 BEGIN/COMMIT) ───────────────────────

/// 事务体:借用连接的 boxed future。`async_trait` 展开会让闭包上下文
/// 高阶化,普通 `FnOnce(&Connection) -> Fut` 过不了生命周期检查,
/// 钉死具体 `'a` 后闭包推断恢复正常。
type TxnBody<'a> = Pin<Box<dyn Future<Output = Result<()>> + Send + 'a>>;

/// 单事务执行 `body`:成功 COMMIT;失败尽力 ROLLBACK 后回传原错误。
async fn run_in_txn<'a>(
    conn: &'a Connection,
    body: impl FnOnce(&'a Connection) -> TxnBody<'a>,
) -> Result<()> {
    conn.execute("BEGIN", ()).await?;
    match body(conn).await {
        Ok(()) => {
            conn.execute("COMMIT", ()).await?;
            Ok(())
        }
        Err(error) => {
            let _ = conn.execute("ROLLBACK", ()).await;
            Err(error)
        }
    }
}

// ─────────────── params_from_iter 的命名转换(闭包在 async_trait 下
// 会触发 HRTB 报错,命名函数对生命周期多态) ───────────────

fn uuid_value(id: &uuid::Uuid) -> Value {
    Value::Text(id.to_string())
}

fn text_value(text: &String) -> Value {
    Value::Text(text.clone())
}

// ───────────────────── torn-WAL 恢复(与 kms 相同的私有副本) ─────────────────────
//
// wiki 刻意不依赖 kms/agentik-core,~50 行帮助函数按 workspace 惯例复制。

/// 线上观察到的真实错误形状:
/// "…I/O error: short read on WAL frame at offset 1751032: expected 4096
/// bytes, got 0"。磁盘满/权限拒绝不得命中。
fn is_torn_wal_error(msg: &str) -> bool {
    msg.contains("short read on WAL frame")
}

/// 把 `db_path` 旁实际存在的 `-wal`/`-shm`/`-twal`/`-tshm` 边车改名成
/// `<边车>.corrupt-<unix-ts>`,让重试从最后一次 checkpoint 的主文件开始,
/// 而不是永远打不开。
fn quarantine_wal_sidecars(db_path: &Path) {
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    for suffix in ["-wal", "-shm", "-twal", "-tshm"] {
        let sidecar = append_suffix(db_path, suffix);
        if !sidecar.is_file() {
            continue;
        }
        let target = append_suffix(&sidecar, &format!(".corrupt-{ts}"));
        match std::fs::rename(&sidecar, &target) {
            Ok(()) => tracing::warn!(
                from = %sidecar.display(),
                to = %target.display(),
                "quarantined torn WAL sidecar"
            ),
            Err(e) => tracing::warn!(
                from = %sidecar.display(),
                to = %target.display(),
                error = %e,
                "failed to quarantine torn WAL sidecar (continuing)"
            ),
        }
    }
}

/// 把 `suffix` 字面追加到 `p` 后,不动已有扩展名。(`Path::with_extension`
/// 会*替换*扩展名,把 `wiki.db` 变成 `wiki-wal`,不是想要的。)
fn append_suffix(p: &Path, suffix: &str) -> PathBuf {
    let mut s = p.as_os_str().to_owned();
    s.push(suffix);
    PathBuf::from(s)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// std-only 版的 `tempfile::tempdir()`:在 `std::env::temp_dir()` 下建
    /// 唯一子目录。
    fn fresh_tmpdir(label: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "wiki-storage-test-{label}-{}-{}",
            std::process::id(),
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn is_torn_wal_error_matches_known_phrase() {
        let msg = "open database: I/O error: short read on WAL frame at offset 1751032: expected 4096 bytes, got 0";
        assert!(is_torn_wal_error(msg));
    }

    #[test]
    fn is_torn_wal_error_rejects_unrelated_io() {
        for msg in [
            "open database: I/O error: disk full",
            "open database: permission denied",
            "open database: database is locked",
        ] {
            assert!(!is_torn_wal_error(msg), "false positive on: {msg}");
        }
    }

    #[test]
    fn quarantine_renames_present_sidecars_only() {
        let tmp = fresh_tmpdir("quarantine_renames_present_sidecars_only");
        let db = tmp.join("wiki.db");
        std::fs::write(&db, b"main-db-bytes").unwrap();
        std::fs::write(append_suffix(&db, "-tshm"), b"shm").unwrap();
        std::fs::write(append_suffix(&db, "-wal"), b"wal").unwrap();

        quarantine_wal_sidecars(&db);

        assert!(db.exists());
        assert_eq!(std::fs::read(&db).unwrap(), b"main-db-bytes");
        assert!(!append_suffix(&db, "-tshm").exists());
        assert!(!append_suffix(&db, "-wal").exists());
        let mut found_tshm = false;
        let mut found_wal = false;
        for entry in std::fs::read_dir(&tmp).unwrap() {
            let name = entry.unwrap().file_name();
            let s = name.to_string_lossy();
            if s.starts_with("wiki.db-tshm.corrupt-") {
                found_tshm = true;
            }
            if s.starts_with("wiki.db-wal.corrupt-") {
                found_wal = true;
            }
        }
        assert!(found_tshm, "expected -tshm.corrupt-* quarantine file");
        assert!(found_wal, "expected -wal.corrupt-* quarantine file");
        std::fs::remove_dir_all(&tmp).ok();
    }

    #[test]
    fn quarantine_is_noop_when_no_sidecars() {
        let tmp = fresh_tmpdir("quarantine_is_noop_when_no_sidecars");
        let db = tmp.join("wiki.db");
        std::fs::write(&db, b"only-main").unwrap();

        quarantine_wal_sidecars(&db);
        assert!(db.exists());
        assert_eq!(std::fs::read(&db).unwrap(), b"only-main");
        std::fs::remove_dir_all(&tmp).ok();
    }

    #[test]
    fn like_escape_covers_wildcards() {
        assert_eq!(like_escape("50%_off\\x"), r"50\%\_off\\x");
    }
}
