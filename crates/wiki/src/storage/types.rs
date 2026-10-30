use super::error::{Error, Result};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};

/// 持久化形状的 wiki 词条。
///
/// 与服务层视图 [`crate::types::KnowledgeEntry`] 的分工:id 是 uuid 的
/// 字符串形式、时间戳是整数;链接(`in_links`/`out_links`)不在词条上,
/// 由 [`WikiIndexStorage`] 的邻接表派生;`tags` 落在独立标签表,读回时
/// 按字典序返回。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KnowledgeEntryData {
    pub id: String,
    pub title: String,
    pub alias: Vec<String>,
    pub tags: Vec<String>,
    pub content: String,
    pub ctime: i64,
    pub mtime: i64,
}

/// 回收站条目:软删除的词条与删除时刻(unix 秒)。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RecycleItem {
    pub entry: KnowledgeEntryData,
    pub deleted_at: i64,
}

/// 版本快照的元信息(不含正文)。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EntryVersionSummary {
    pub version: i64,
    pub created_at: i64,
}

/// 检索过滤器:全部条件取交集。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SearchFilter {
    /// 命中需**全部**含有的标签(AND;trim 后精确匹配,不折叠大小写)。
    pub tags: Vec<String>,
    /// `mtime` 下界(unix 秒,含)。
    pub mtime_since: Option<i64>,
    /// `mtime` 上界(unix 秒,含)。
    pub mtime_until: Option<i64>,
}

/// 一次检索命中:词条 id 与 BM25 得分。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchHit {
    pub entry_id: String,
    pub score: f64,
}

#[async_trait]
pub trait WikiEntryStorage: Send + Sync {
    /// 新建条目。title 先 trim 再落库;trim 后为空返回
    /// [`Error::EmptyTitle`],与现存条目重复(trim 后精确匹配,**含回收
    /// 站中未净删的条目**)返回 [`Error::DuplicateTitle`]。同时建立检索
    /// 倒排、别名/标签索引、出边(从 content 提取 wiki 链接)与 v1 版本
    /// 快照,全部同事务。不会有部分写入。
    async fn create_entry(&self, entry: KnowledgeEntryData) -> Result<()>;
    /// 软删除进回收站:设 `deleted_at`,词条从读取/检索/邻接中隐去;
    /// 标题仍被占用,直到 [`WikiEntryStorage::purge_entry`] 或
    /// [`WikiEntryStorage::restore_entry`]。id 不存在时是幂等 no-op。
    async fn delete_entry(&self, id: uuid::Uuid) -> Result<()>;
    /// 从回收站恢复:`deleted_at` 置空,索引与链接原样可用(未净删时
    /// 从不清除)。id 不存在返回 [`Error::NotFound`];已在回收站外是
    /// 幂等 no-op。
    async fn restore_entry(&self, id: uuid::Uuid) -> Result<()>;
    /// 净删:从回收站彻底移除词条及其全部索引行(倒排/别名/标签/版本/
    /// 双向链接),标题随即释放。id 不存在时是幂等 no-op。
    async fn purge_entry(&self, id: uuid::Uuid) -> Result<()>;
    /// 回收站清单,按删除时间升序。
    async fn list_deleted(&self) -> Result<Vec<RecycleItem>>;
    /// 按 id 取单条(回收站中的词条视为不存在);不存在时返回
    /// [`Error::NotFound`]。
    async fn get_entry(&self, id: uuid::Uuid) -> Result<KnowledgeEntryData>;
    /// 按标题精确查找(未删除词条);不存在返回 `None`。工具层的
    /// title 引用解析走这里。
    async fn find_by_title(&self, title: &str) -> Result<Option<KnowledgeEntryData>>;
    /// 批量取条目:只返回存在且未删除的,按入参顺序输出,重复 id 只
    /// 出现一次。
    async fn get_entry_batch(&self, ids: Vec<uuid::Uuid>) -> Result<Vec<KnowledgeEntryData>>;
    /// 覆盖式更新单条(含 `ctime`,调用方负责保全);title 同样先 trim
    /// 校验。id 不存在(或已软删)时返回 [`Error::NotFound`];重命名为
    /// 其他条目占用的标题返回 [`Error::DuplicateTitle`],重命名为自身
    /// 现名允许。每次更新重建倒排/别名/标签/出边并追加版本快照。
    async fn update_entry(&self, entry: KnowledgeEntryData) -> Result<()>;
    /// 批量覆盖式更新:单事务包裹,任一条失败(含规范化失败)则全部
    /// 回滚/拒绝。
    async fn update_entry_batch(&self, entries: &[KnowledgeEntryData]) -> Result<()>;
    /// 版本历史(新→旧),只有元信息;正文用
    /// [`WikiEntryStorage::get_version`] 取。
    async fn list_versions(&self, id: uuid::Uuid) -> Result<Vec<EntryVersionSummary>>;
    /// 取某版快照的完整词条数据。回滚 = `get_version` 后走
    /// `update_entry`(会追加新版本,历史不丢)。
    async fn get_version(&self, id: uuid::Uuid, version: i64) -> Result<KnowledgeEntryData>;
}

/// 链接索引(有向邻接表)的读取。
///
/// 所有 batch 方法遵循同一契约:返回值与 `ids` 等长且下标对齐 ——
/// `result[i]` 是 `ids[i]` 这个词条的邻居列表;入参重复则结果重复,
/// 无边的 id 得到空桶,`ids` 为空时返回空。**回收站中的词条不作为
/// 邻居出现。**
#[async_trait]
pub trait WikiIndexStorage: Send + Sync {
    /// 指向 `ids` 中各词条的来源词条 id(谁链接到我)。
    async fn get_in_links_batch(&self, ids: &[String]) -> Result<Vec<Vec<String>>>;
    /// `ids` 中各词条指向的目标词条 id(我链接到谁)。
    async fn get_out_links_batch(&self, ids: &[String]) -> Result<Vec<Vec<String>>>;
    /// 提及:正文里以纯文本出现过该词条标题或任一别名(且尚未以
    /// wiki 链接指向它)的其他未删除词条 id,按 id 升序。这是补链
    /// (auto-link)的候选来源。
    async fn get_mentions(&self, id: &str) -> Result<Vec<String>>;
}

/// 全文检索能力:BM25,中英文混合分词(见 [`crate::search`])。
#[async_trait]
pub trait WikiSearchStorage: Send + Sync {
    /// 等价于 [`WikiSearchStorage::search_with_filter`] 加空过滤器。
    async fn search(&self, query: &str, limit: usize) -> Result<Vec<SearchHit>> {
        self.search_with_filter(query, limit, &SearchFilter::default())
            .await
    }
    /// 按 BM25 相关性降序返回至多 `limit` 个命中(回收站词条不参与,
    /// 统计口径同样只算未删除词条);空查询或 `limit == 0` 返回空。
    /// 同分以 entry_id 升序保证确定性。
    async fn search_with_filter(
        &self,
        query: &str,
        limit: usize,
        filter: &SearchFilter,
    ) -> Result<Vec<SearchHit>>;
}

/// 存储后端的组合 trait:同时提供词条存储、索引与检索能力。
///
/// Rust 的 trait object 只允许「一个普通 trait + 任意多个 auto trait」,
/// 因此 `dyn WikiEntryStorage + WikiIndexStorage` 会被 E0225 拒绝。
/// 这里用 supertrait 把各能力合成单一 trait,`dyn WikiStorage` 就合法了。
pub trait WikiStorage: WikiEntryStorage + WikiIndexStorage + WikiSearchStorage {}

/// blanket impl:任何同时实现了全部基础 trait 的类型自动满足
/// `WikiStorage`,实现方无需再手写一遍 `impl WikiStorage for ...`。
impl<T: WikiEntryStorage + WikiIndexStorage + WikiSearchStorage> WikiStorage for T {}
