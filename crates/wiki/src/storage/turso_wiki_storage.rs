//! Turso 后端:工作区既定的嵌入式打开方式 + 手写行解析。
//!
//! 与 kms / writing-base / bib-base 同款形态:`Builder::new_local` 打开,
//! 非内存库启用 `experimental_multiprocess_wal`(daemon 与 TUI 并发持库)
//! 并设 5 s busy timeout;连接收进 `tokio::sync::Mutex` 串行化访问;多
//! 语句写路径用显式 BEGIN/COMMIT 包裹,失败 ROLLBACK。行解析手写 ——
//! turso 不提供 `FromRow` 派生(与 kms 相同的取舍)。
//!
//! 链接索引落在 `entry_links` 有向边表上:出边的写入入口是
//! [`TursoWikiStorage::replace_out_links`],`wiki::markdown` 的链接提取
//! 落地后,服务层在写条目后调它重建该条目的出边。
//!
//! 全文检索(turso 无 FTS,同 bib-base 自建倒排):`search_postings` 存
//! (term, entry, tf),`search_docs` 存文档 token 长度;写路径与条目
//! 同事务维护,查询侧拉取候选 postings 在 Rust 里按 BM25 打分。

use std::collections::{HashMap, HashSet};
use std::future::Future;
use std::path::{Path, PathBuf};
use std::pin::Pin;

use async_trait::async_trait;
use tokio::sync::Mutex;
use turso::{Connection, Value};

use super::error::{Error, Result};
use super::types::{
    KnowledgeEntryData, SearchHit, WikiEntryStorage, WikiIndexStorage, WikiSearchStorage,
};
use crate::search::{bm25_term, idf, tokenize};

/// `IN (...)` 查询单块的参数上限(SQLite 系绑定变量上限 999;80 与
/// bib-base 的分块粒度一致)。
const IN_CHUNK: usize = 80;

const SCHEMA_SQL: &str = "\
CREATE TABLE IF NOT EXISTS entries (
    id      TEXT PRIMARY KEY NOT NULL,
    title   TEXT NOT NULL,
    alias   TEXT NOT NULL DEFAULT '[]',
    content TEXT NOT NULL,
    ctime   INTEGER NOT NULL,
    mtime   INTEGER NOT NULL,
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
    length INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS search_postings (
    term TEXT NOT NULL,
    entry_id TEXT NOT NULL,
    tf INTEGER NOT NULL,
    PRIMARY KEY (term, entry_id)
);

CREATE INDEX IF NOT EXISTS idx_search_postings_entry ON search_postings(entry_id);
";

/// Turso 存储后端。
pub struct TursoWikiStorage {
    conn: Mutex<Connection>,
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
        let conn = db.connect()?;
        if path != ":memory:" {
            conn.pragma_update("busy_timeout", 5000).await?;
        }
        let storage = Self {
            conn: Mutex::new(conn),
        };
        storage.init_schema().await?;
        Ok(storage)
    }

    async fn init_schema(&self) -> Result<()> {
        let conn = self.conn.lock().await;
        conn.execute_batch(SCHEMA_SQL).await?;
        Ok(())
    }

    /// 覆盖式重写 `from` 的全部出边。
    ///
    /// `wiki::markdown` 链接提取落地后,这是服务层重建出边索引的写入
    /// 点;当前也可直接用于测试与工具。单事务:旧边删净、新边
    /// `INSERT OR IGNORE` 入库,重复目标自动去重。
    pub async fn replace_out_links(&self, from: &str, targets: &[String]) -> Result<()> {
        let conn = self.conn.lock().await;
        run_in_txn(&conn, |conn| {
            Box::pin(async move {
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
                index_entry(conn, &entry).await?;
                Ok(())
            })
        })
        .await
    }

    async fn delete_entry(&self, id: uuid::Uuid) -> Result<()> {
        let id = id.to_string();
        let conn = self.conn.lock().await;
        run_in_txn(&conn, |conn| {
            Box::pin(async move {
                let id2 = id.clone();
                conn.execute(
                    "DELETE FROM entry_links WHERE from_id = ?1 OR to_id = ?1",
                    turso::params![id2],
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
                conn.execute("DELETE FROM entries WHERE id = ?1", turso::params![id])
                    .await?;
                Ok(())
            })
        })
        .await
    }

    async fn get_entry(&self, id: uuid::Uuid) -> Result<KnowledgeEntryData> {
        let conn = self.conn.lock().await;
        let mut rows = conn
            .query(
                "SELECT id, title, alias, content, ctime, mtime FROM entries WHERE id = ?1",
                turso::params![id.to_string()],
            )
            .await?;
        match rows.next().await? {
            Some(row) => entry_from_row(&row),
            None => Err(Error::NotFound(id.to_string())),
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
                 WHERE id IN ({placeholders})"
            );
            let mut rows = conn
                .query(&sql, turso::params_from_iter(chunk.iter().map(uuid_value)))
                .await?;
            while let Some(row) = rows.next().await? {
                let entry = entry_from_row(&row)?;
                by_id.insert(entry.id.clone(), entry);
            }
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
                index_entry(conn, &entry).await?;
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
                    index_entry(conn, entry).await?;
                }
                Ok(())
            })
        })
        .await
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
}

#[async_trait]
impl WikiSearchStorage for TursoWikiStorage {
    async fn search(&self, query: &str, limit: usize) -> Result<Vec<SearchHit>> {
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

        // 全库统计:N 与平均文档长度(以 search_docs 为准,空库直接空)。
        let mut stats_rows = conn
            .query(
                "SELECT COUNT(*), COALESCE(AVG(length), 0.0) FROM search_docs",
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

        // 候选 postings:查询词的全部 (term, entry, tf)。
        let mut tf: HashMap<String, HashMap<String, i64>> = HashMap::new(); // entry -> term -> tf
        let mut df: HashMap<String, usize> = HashMap::new(); // term -> 含该词文档数
        for chunk in terms.chunks(IN_CHUNK) {
            let placeholders = (1..=chunk.len())
                .map(|position| format!("?{position}"))
                .collect::<Vec<_>>()
                .join(", ");
            let sql = format!(
                "SELECT term, entry_id, tf FROM search_postings WHERE term IN ({placeholders})"
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

        // 候选文档长度。
        let candidates: Vec<String> = tf.keys().cloned().collect();
        let mut doc_len: HashMap<String, i64> = HashMap::new();
        for chunk in candidates.chunks(IN_CHUNK) {
            let placeholders = (1..=chunk.len())
                .map(|position| format!("?{position}"))
                .collect::<Vec<_>>()
                .join(", ");
            let sql = format!(
                "SELECT entry_id, length FROM search_docs WHERE entry_id IN ({placeholders})"
            );
            let mut rows = conn
                .query(&sql, turso::params_from_iter(chunk.iter().map(text_value)))
                .await?;
            while let Some(row) = rows.next().await? {
                doc_len.insert(text_col(&row, 0)?, int_col(&row, 1)?);
            }
        }

        // BM25 打分:score(d) = Σ_t idf(t) · tf 部分。
        let mut hits: Vec<SearchHit> = tf
            .into_iter()
            .filter_map(|(entry_id, term_tf)| {
                let dl = doc_len.get(&entry_id).copied().unwrap_or(0);
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

// ─────────────────────── 条目行读写(自由函数,单/批量共用) ───────────────────────

/// 写入前规范化与校验:trim 首尾空白;全空白标题拒绝。
/// 不折叠大小写——`BRCA1` 与 `brca1` 对专业缩写是不同词条,保持区分。
fn normalize_entry(entry: KnowledgeEntryData) -> Result<KnowledgeEntryData> {
    let title = entry.title.trim().to_string();
    if title.is_empty() {
        return Err(Error::EmptyTitle);
    }
    Ok(KnowledgeEntryData { title, ..entry })
}

/// 事务内预检标题占用(trim 后精确匹配):命中返回占用者 id;
/// `exclude_id` 跳过自身(重命名为现名不算占用)。预检只求友好错误,
/// 并发窗口仍由 `UNIQUE(title)` 约束兜底。
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
             WHERE id = ?1",
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

/// 重建 `entry` 的检索倒排:文档 = title + alias + content 拼接后分词。
/// 幂等:先清该条目旧 postings 与长度行再写。须在调用方事务内执行,
/// 与条目行保持原子。
async fn index_entry(conn: &Connection, entry: &KnowledgeEntryData) -> Result<()> {
    conn.execute(
        "DELETE FROM search_postings WHERE entry_id = ?1",
        turso::params![entry.id.clone()],
    )
    .await?;
    conn.execute(
        "DELETE FROM search_docs WHERE entry_id = ?1",
        turso::params![entry.id.clone()],
    )
    .await?;

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
    Ok(())
}

/// 邻接表批量查询:`WHERE {key} IN (...)`,把 `{value}` 按 key 分桶。
/// 桶内按 value 排序保证确定性;`key`/`value` 是编译期列名,非用户输入。
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
            "SELECT {key}, {value} FROM entry_links WHERE {key} IN ({placeholders}) \
             ORDER BY {value}"
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
}
