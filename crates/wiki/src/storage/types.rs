use super::error::{Error, Result};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};

/// 持久化形状的 wiki 条目。
///
/// 与服务层视图 [`crate::types::KnowledgeEntry`] 的分工:id 是 uuid 的
/// 字符串形式、时间戳是整数;链接(`in_links`/`out_links`)不在条目上,
/// 由 [`WikiIndexStorage`] 的邻接表派生。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KnowledgeEntryData {
    pub id: String,
    pub title: String,
    pub alias: Vec<String>,
    pub content: String,
    pub ctime: i64,
    pub mtime: i64,
}

#[async_trait]
pub trait WikiEntryStorage: Send + Sync {
    /// 新建条目。title 先 trim 再落库;trim 后为空返回
    /// [`Error::EmptyTitle`],与现存条目重复(trim 后精确匹配)返回
    /// [`Error::DuplicateTitle`]。不会有部分写入。
    async fn create_entry(&self, entry: KnowledgeEntryData) -> Result<()>;
    /// 删除条目,并清掉它在链接表两个方向上的所有边;id 不存在时是
    /// 幂等 no-op。
    async fn delete_entry(&self, id: uuid::Uuid) -> Result<()>;
    /// 按 id 取单条;不存在时返回 [`Error::NotFound`]。
    async fn get_entry(&self, id: uuid::Uuid) -> Result<KnowledgeEntryData>;
    /// 批量取条目:只返回存在的,按入参顺序输出,重复 id 只出现一次。
    async fn get_entry_batch(&self, ids: Vec<uuid::Uuid>) -> Result<Vec<KnowledgeEntryData>>;
    /// 覆盖式更新单条(含 `ctime`,调用方负责保全);title 同样先 trim
    /// 校验。id 不存在时返回 [`Error::NotFound`];重命名为其他条目占用
    /// 的标题返回 [`Error::DuplicateTitle`],重命名为自身现名允许。
    async fn update_entry(&self, entry: KnowledgeEntryData) -> Result<()>;
    /// 批量覆盖式更新:单事务包裹,任一条失败(含规范化失败)则全部
    /// 回滚/拒绝。
    async fn update_entry_batch(&self, entries: &[KnowledgeEntryData]) -> Result<()>;
}

/// 链接索引(有向邻接表)的读取。
///
/// 所有 batch 方法遵循同一契约:返回值与 `ids` 等长且下标对齐 ——
/// `result[i]` 是 `ids[i]` 这个条目的邻居列表;入参重复则结果重复,
/// 无边的 id 得到空桶,`ids` 为空时返回空。
#[async_trait]
pub trait WikiIndexStorage: Send + Sync {
    /// 指向 `ids` 中各条目的来源条目 id(谁链接到我)。
    async fn get_in_links_batch(&self, ids: &[String]) -> Result<Vec<Vec<String>>>;
    /// `ids` 中各条目指向的目标条目 id(我链接到谁)。
    async fn get_out_links_batch(&self, ids: &[String]) -> Result<Vec<Vec<String>>>;
}

/// 一次检索命中:条目 id 与 BM25 得分。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchHit {
    pub entry_id: String,
    pub score: f64,
}

/// 全文检索能力:BM25,中英文混合分词(见 [`crate::search`])。
#[async_trait]
pub trait WikiSearchStorage: Send + Sync {
    /// 按 BM25 相关性降序返回至多 `limit` 个命中;空查询或 `limit == 0`
    /// 返回空。同分以 entry_id 升序保证确定性。
    async fn search(&self, query: &str, limit: usize) -> Result<Vec<SearchHit>>;
}

/// 存储后端的组合 trait:同时提供条目存储、索引与检索能力。
///
/// Rust 的 trait object 只允许「一个普通 trait + 任意多个 auto trait」,
/// 因此 `dyn WikiEntryStorage + WikiIndexStorage` 会被 E0225 拒绝。
/// 这里用 supertrait 把各能力合成单一 trait,`dyn WikiStorage` 就合法了。
pub trait WikiStorage: WikiEntryStorage + WikiIndexStorage + WikiSearchStorage {}

/// blanket impl:任何同时实现了全部基础 trait 的类型自动满足
/// `WikiStorage`,实现方无需再手写一遍 `impl WikiStorage for ...`。
impl<T: WikiEntryStorage + WikiIndexStorage + WikiSearchStorage> WikiStorage for T {}
