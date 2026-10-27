# 嵌套 Collection 迁移分析

本方案基于 2026-10-27 工作区代码（包括已有未提交修改）及当日 Zotero `main` 源码。本文保留最初的设计分析；当前实施和验证情况见 [迁移实施说明](zotero-collections-implementation.md)。目标是先实现与 Zotero 一致的嵌套组织语义，再逐步接入已有 item 模型。

## 1. 当前实现和缺口

| 层 | 当前实现 | 迁移影响 |
| --- | --- | --- |
| 类型 | `crates/bib-types/src/types.rs::Collection` 只有 id、name、description、article_ids、tags、status 和时间 | 增加可空 parent_id；旧 JSON 缺省为根节点 |
| 数据库 | `crates/bib-base/src/bib_base.rs::SCHEMA_SQL` 的 collections 无父关系；collection_articles 为多对多关联 | 增量添加父引用和索引，保留全部关联元数据 |
| 存储 | `crates/bib-base/src/collections.rs` 实现 CRUD、关联和批量读取；列表按 updated_at 倒序 | 增加移动、子树遍历和递归查询；树内同级按名称排序 |
| 并发 | `crates/bib-base/src/actor.rs` 提供 BibBase → BibCore 命令分派；读写使用不同连接 | 所有结构变更必须走 actor，在同一事务内校验并写入 |
| API | `crates/api-server/src/bib.rs` 支持创建、获取、删除、改 status 和关联操作 | 没有 rename/move，创建没有 parent 参数，文献读取只有直接成员 |
| 主 GUI | `apps/gui/src/features/bibliography.tsx` 使用 collections.map 和 SidebarItem 平铺 | 新增树视图及创建、重命名、移动、删除入口；类型在 apps/gui/src/types.ts |
| 另一套 Web UI | `crates/api-server/frontend/src/components/bibliography/collection-tree.tsx` 名为 Tree，实际平铺 | 还需修改 app.tsx 的创建、删除、查询和详情中的 collection 选择器 |
| Agent | `crates/bib-base/src/library_tools.rs` 的 create 输入没有 parent；list 输出手写 JSON | 即使 Collection 增加字段，工具输出也不会自动带上 parent_id |
| TUI | `apps/autonomics/src/widgets/bibliography/collection_pane.rs` 平铺；相关导航状态在 bibliography 模块 | 更新树的可见行及键盘索引，避免渲染顺序与选择顺序错位 |

实际 gateway 根页面挂载的是 `apps/gui` 构建到 `crates/gateway/gui-dist` 的资源，见 `crates/gateway/src/server/router.rs::router_with_bib`、`server/gui.rs` 和 `apps/gui/README.md`。实施应以 apps/gui 为主，另一套 UI 做兼容适配。

`crates/bib-base/src/zotero_model.rs` 已有 libraries、items、articleItems、附件和批注模型，但没有 Zotero collectionItems，也没有 collection 的 library/key 身份模型。当前 collection 仍完全属于旧模型。

已有测试 `bib_collections_and_annotations_use_nested_routes` 中的 nested 指 URL 层级，未测试 collection 父子关系。

## 2. Zotero 参考依据

直接参考以下上游实现；main 会变化，正式实施时应固定 commit：

- [userdata.sql](https://github.com/zotero/zotero/blob/main/resource/schema/userdata.sql)：collections 使用 parentCollectionID 自引用，collectionItems 独立保存条目归属；collection 拥有 libraryID/key 身份。
- [collection.js](https://github.com/zotero/zotero/blob/main/chrome/content/zotero/xpcom/data/collection.js)：`_initSave` 校验父节点，禁止自身和后代作为父节点；`getDescendents` 遍历子树；`trash` 与 `_eraseData` 区分回收站和永久清除。
- [collections.js](https://github.com/zotero/zotero/blob/main/chrome/content/zotero/xpcom/data/collections.js)：`getByLibrary`、`getByParent` 分别访问库和子集合，并以 locale collation 排列同级。
- [collectionTree.jsx](https://github.com/zotero/zotero/blob/main/chrome/content/zotero/collectionTree.jsx)：可见行、行 ID 索引、持久化展开状态、拖拽校验、选中节点祖先展开和通知驱动更新。
- [官方使用说明](https://www.zotero.org/support/collections_and_tags)：多重归属、创建子集合、拖拽、删除子树及“Show Items from Subcollections”的用户语义。

借鉴模型与行为，在现有 Rust/React 架构中实现；本方案不要求移植 Zotero 的 XPCOM 或桌面组件。

## 3. 必须先固定的行为契约

1. Collection 构成森林，每个节点至多有一个父节点；根节点 parent_id 为 null。
2. 文献独立存在，一篇文献可以属于多个 collection。移动集合不移动文件、不复制文献、不改变关联元数据。
3. 子集合的文献不会自动写入祖先的成员表。直接归属和递归展示是两个概念。
4. 默认只显示直接成员，提供“显示子集合文献”开关；切换开关只影响查询范围。
5. 删除集合作用于整个子树及其归属关系，文献、附件和批注保留。不要默认开放“删除集合及其文献”。
6. Unfiled 指没有任何有效 collection 直接归属的文献；仅属于子集合的文献也已归档。
7. Collection 的 status 仍是节点本身的研究状态；不能把 archived 当作 trash，也不隐式继承到后代。
8. 允许重名，UI 用祖先路径消歧，引用始终用 ID；不从名称、斜杠路径或 tags 推断层级。

其中第 4 点采用 Zotero 文档明确描述的默认行为。当前上游还实现了 collection 回收站；第一阶段若保留本项目现有永久删除，应在 UI 明示，不能宣称已完整复刻 Zotero 删除生命周期。

## 4. 存储迁移：先复用现有表

第一阶段建议仅扩展当前 collections，避免在嵌套功能中同时引入两套 collection 真相源：

```sql
ALTER TABLE collections
    ADD COLUMN parent_id TEXT REFERENCES collections(id) ON DELETE CASCADE;
CREATE INDEX IF NOT EXISTS idx_collections_parent_id ON collections(parent_id);
```

这是目标 DDL，需要先验证项目实际 turso 版本对 ALTER、自引用 FK、递归 CTE 和事务 DDL 的支持。不能用 Python sqlite 测试结果代替运行时验证。新库 SCHEMA_SQL 与旧库迁移应得到相同结构。

迁移流程：检测列 → 在写连接事务内新增缺失列 → 建索引 → 校验 → 记录迁移版本。全部已有集合自然成为根节点；不更改 ID、article_ids 对应关系、position、role、fetch_status、added_by、note 和 added_at。重复打开必须不改变用户之后建立的父关系。

注意现有 migrate 会重复执行幂等 DDL，还涉及读连接 schema 可见性。父列索引必须在旧库补列后创建，不能提前放进针对旧表执行的基础 DDL 中。事务失败应完整回滚；发布前使用离线备份或数据库支持的一致性备份验证恢复路径，不直接复制一个正在写入的 bib.db 当作可靠备份。

类型新增 `#[serde(default)] parent_id: Option<String>`；构造器缺省 None。禁止旧客户端用缺失父字段的完整 upsert 把已有节点移回根。对外使用专门的 create/patch/move 操作；审计所有内部 upsert 调用，保留读取对象的父字段。

新增核心方法：

```text
create_collection(input)
rename_collection(id, name)
move_collection(id, parent_id)
collection_descendant_ids(id)
list_collection_articles_scoped(id, include_descendants, query, pagination)
delete_collection_subtree(id)
```

校验和写入必须在一个 actor 命令及事务内完成，覆盖父节点不存在、自引用、多层环、空名称、源节点不存在。未来增加多库后再补同库和权限约束。相同父节点的重复 move 是无副作用成功。不要通过 API 先查父节点、再另发写入命令来保证完整性。

向上遍历拟设父节点的祖先即可检测环；若发现已有损坏环，返回结构错误。递归 SQL 可以用 UNION 去重避免在损坏数据上无限遍历，不能仅依赖前端禁用拖拽。删除可以依赖经过测试的 FK 级联；也可在事务中显式删除子树，仍须返回受影响 ID 供客户端更新状态。

## 5. 查询及关联元数据

保留现有 `Collection.article_ids` 为直接成员，不能悄悄改成子树成员。现有调用方用它判断真实归属和执行移除。

递归查询先取集合 ID 闭包，再读取归属和文献；文献结果按 article_id 去重，再排序和分页。不能把每个子集合先分页后拼接，也不能把子集合计数相加当作去重文献数。使用现有批量 hydration 路径，避免逐篇 get_article。

特别注意本项目的 CollectionArticle 比 Zotero 的 collectionItems 更丰富：同一篇文献在 A、A/B 中可能拥有不同 role、note 和 fetch_status。递归模式保留每条关联的来源 collection_id，articles 去重，但 associations 不合并。返回 direct_member 和来源集合信息，让操作明确作用于哪一条直接关联。

在父集合递归视图看到只属于后代的文献时，不能直接显示“从当前集合移除”并发送一个实际上没有效果的请求。应提供“管理所属集合”，展示真实来源路径；直接成员可提供从当前集合移除。

新树接口返回 direct_article_count、child_count，递归数按需计算。旧列表保留 article_ids 兼容；主 GUI 切换轻量 summary 查询，避免每次翻页都重取全部集合及所有成员 ID。Unfiled 应由后端全库 NOT EXISTS 查询和 count 计算，不能从分页加载的文献中推断。

全文搜索必须在后端先应用 collection 范围，再执行搜索、计数和分页。当前 GUI 集合内只做标题等字段的本地筛选，另一套 UI 通过全库最多 500 个命中求交；这两条路径都不适合扩大后的递归集合。

## 6. API 与工具契约

保留现有 `/api/v1/bib` 前缀和 articles 术语，不要求照搬 Zotero API 命名。

| 接口 | 目标行为 |
| --- | --- |
| GET /collections | 兼容旧列表；增加 parent_id |
| GET /collections?view=summary | 新树使用轻量元数据与直接计数，不返回全部 article_ids |
| POST /collections | 接受 name、parent_id；缺省 null |
| PATCH /collections/{id} | 修改名称或父节点；未提供字段保持原值，显式 parent_id:null 才移到根 |
| GET /collections/{id}/articles | 增加 include_descendants=false、query、limit、offset，返回去重 total 和关联来源 |
| DELETE /collections/{id} | 删除整棵子树，返回 deleted_ids；保留文献 |

PATCH 必须区分“缺少字段”和“显式 null”，普通 Option<String> 的默认反序列化不足以表达此区别，需自定义 presence 类型或等效三态反序列化。父字段的校验统一交给核心层。错误应区分 404 节点缺失、400 参数不合法、409 循环/结构冲突，而不是全部通过 internal 映射成 500。

Agent 的 bib_create_collection 增加 parent_id；bib_list_collection 增加 parent_id/path/direct_count 和显式递归范围；提供 move/rename 管理工具。bib_save 沿用 collection_id，不必理解路径。导出、全文请求列表及 CLI 的 collection 过滤默认保留直接范围，若新增递归必须显式传参并复用同一查询语义。

## 7. 主前端改造

建议从 BibliographyView 抽出独立 CollectionTree、CollectionDialog 和树索引工具：

```text
collectionsById + childrenByParent
          ↓ 同级名称排序、展开状态
visibleRows: { id, depth, parentId, hasChildren, expanded }[]
```

树索引构建 O(n)，按同级排序；中文及数字名称用 Intl.Collator，名称相同用 ID 保持稳定。第一版可普通渲染，只有实际规模需要时才引入虚拟化。

- 根“全部文献”作为特殊视图，不写进 collections 表；Unfiled 同样是虚拟视图。
- 展开按钮只控制折叠，点击行才选择集合。展开状态按数据库/用户/library 身份存入 localStorage，选择状态按 ID 保持。
- 用 role=tree/treeitem、aria-level、aria-expanded、aria-selected 和 roving tabindex；支持上下移动、左右展开/返回父节点、Home/End、F2、右键菜单。
- 菜单提供新建子集合、重命名、移动到、删除；“移动到”对话框与拖拽走同一 API，移动选项排除自身和后代。
- 拖 collection 到 collection 是改父节点，拖到库根是移至根；拖文献默认添加归属。显式移动文献需要事务性 add+remove，保留来源语义。
- 新建后展开祖先并选中新节点；移动后保持选中 ID，展开目标祖先。折叠包含选中节点的祖先时需将焦点移到可见祖先，避免不可见焦点。
- 删除确认展示子集合数量并说明文献保留；成功后若选中 ID 在 deleted_ids 中，回到最近存活祖先或全部文献。
- 所有选择器及详情归属使用路径标签，如 ALS / Methods / Single-cell；路径只是派生显示值。
- 显示子集合文献开关属于视图偏好，不写入 collection 元数据。搜索、计数和导出明确使用当前范围。
- 导入时传入当前选择的真实 collection_id，由后端在文献入库与归属写入的同一事务中完成。当前主 GUI 导入没有关联当前集合，迁移时需要补齐。

请求状态也应一起整理：用 view + query + recursive + pagination 作为统一请求键，并配合请求序号或取消。当前 loadLibrary 和 loadCollectionArticles 各有序号，但共用 loading、attachmentSummaries，切换根视图不会取消旧集合请求。选择新集合时先清空或隐藏旧列表；移动/删除/归属变更成功后刷新树、计数和受影响视图。失败时保留对话框和输入，不能提前关闭造成成功假象。

初版可在本地操作成功和页面重新获得焦点时重新校验数据；若要让 Agent 和多个窗口的修改实时可见，再通过现有事件基础设施新增 collection/membership 变更事件，不能假设现在已经有 Zotero Notifier 的等价能力。

另一套 Web UI 的删除回退也要基于 deleted_ids，不能只判断 activeView 是否等于被删根节点；TUI 则复用相同树顺序和可见行模型。

## 8. 实施顺序与验收

1. 核心类型、增量迁移、移动校验、删除子树、actor 委托及存储测试。
2. API、错误映射、递归查询与直接/来源关联契约；同步 Agent 工具。
3. apps/gui 树、新建/重命名/移动/删除、持久化展开状态与键盘操作。
4. 递归开关、搜索分页、路径选择器、导入归属；兼容另一套 Web UI 和 TUI。
5. GUI 构建及 gateway 静态资源同步；验证实际 gateway 和 Tauri 两种入口。
6. 后续独立迁移到 library/key、collectionItems 和 collection trash。

必须通过的行为验收：

- 旧库打开后原集合均为根，全部语义元数据保留；重复迁移不丢父关系。
- A → B → C 建立后可移动 B 到根，再移回 A；三层结构和成员完整保留。
- 拒绝 A → A、A → C、缺失父节点；失败后数据库与 UI 保持原状态。
- 同篇文献属于父子及其他分支，直接视图正确，递归视图只显示一次；来源关联全部保留。
- 删除 B 删除 B/C 及其归属，文献/附件/批注和其他集合归属保留；选中 C 时正确回退。
- 子集合内文献不出现在 Unfiled；计数不受当前分页或搜索结果影响。
- 递归全文搜索中超过 500 条时分页完整、计数准确、无重复遗漏。
- 快速切换集合/根节点及递归开关时，旧请求不覆盖新视图。
- Agent 创建子集合、GUI 移动、TUI 查看和重启后都得到一致结构。

现阶段只编写设计文档，未运行上述尚未实现的功能测试。

## 9. 与现有 Zotero item 迁移的后续衔接

第二阶段再决定 collections 的最终物理表结构：可通过重建表及兼容映射引入 collectionID、libraryID、key、parentCollectionID；公开 API 的既有 col-* ID 必须保留稳定映射，不将内部主键变化暴露给旧调用方。

collectionItems 通过 articleItems 连接顶层文献 item。role/note/fetch_status 等作为 Autonomics 扩展保留，不能迁成只有 collectionID/itemID 的表后丢弃。迁移期间若暂时保留旧关联表，明确单一真相源，所有必要镜像写入在同一事务完成，并提供一致性校验。

回收站阶段增加 deletedCollections 或等价 tombstone，默认查询排除已删除子树；再定义恢复时父节点仍在回收站或已永久删除的行为。同步、跨库复制及版本冲突属于后续独立能力，增加字段本身不等于实现 Zotero 同步。
