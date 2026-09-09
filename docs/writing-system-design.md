# LaTeX 写作系统设计

> 状态：设计稿 (2026-08-09)
> 关联：`bib-types` / `bib-base` 文献管理 · `dag-core` DAG 引擎 · `agentik-core` Agent 工具框架

---

## 1. 目标与范围

在现有文献管理系统（`bib-base`：Article / Collection / Annotation / 全文 / OA 抓取 / BibTeX 导出）
之上，构建一套 **以 Agent 为一等公民** 的 LaTeX 学术写作系统。

### 核心需求

| 需求 | 说明 |
|------|------|
| **与文献管理深度融合** | 引文键自动从 `bib-base` 解析；.bib 文件自动生成；文献库增删时引文一致性检查 |
| **结构化标引** | 文档内部不是平铺文本，而是语义块树（段落/公式/图表/声明）；声明可挂载证据链（引文 + 数据） |
| **引文自动管理** | 引用风格切换 (natbib / biblatex)；断引检测；引文图谱（哪段话引了哪篇文） |
| **Agent 优化编辑** | AST 级语义操作（插入段落 / 移动章节 / 插入引文），而非文本 diff；稳定 block ID；可组合编辑脚本 |
| **数据 & 图表插入** | DAG DataFrame → LaTeX tabular；可视化 artifact → `\includegraphics`；VFS 查询 → 内联表格 |
| **LaTeX 编译** | 可插拔引擎（Tectonic 内嵌 / XeLaTeX 子进程），模板系统，实时编译反馈 |

### 非目标

- 不做 WYSIWYG 编辑器（TUI / Web 前端是后续话题）
- 不替代通用 LaTeX IDE（Overleaf, TeXstudio）
- 不实现完整 LaTeX 解析器（只解析我们生成的子集 + 常见 raw 块透传）

---

## 2. Crate 结构

遵循 `bib-types` / `bib-base` 的两层模式：

```
crates/
├── writing-types/          # 纯数据类型，零 I/O，零副作用
│   └── src/
│       ├── lib.rs
│       ├── document.rs     # Document, Section, Block, Inline
│       ├── citation.rs     # CiteKey, CitationCluster, CitationStyle
│       ├── claim.rs        # Claim, EvidenceLink, EvidenceType
│       ├── figure.rs       # FigureSpec, TableSpec, SubfigureSpec
│       ├── template.rs     # DocumentClass, TemplateSpec
│       └── edit.rs         # EditOp, EditScript, DocumentDiff
│
├── writing-base/           # 存储 + 文档模型操作 + LaTeX 序列化 + 引文解析
│   └── src/
│       ├── lib.rs
│       ├── store.rs        # WritingStore (Turso) — 文档 CRUD + 版本
│       ├── ast.rs          # AST 操作：查找 / 插入 / 删除 / 移动 block
│       ├── serialize.rs    # AST → LaTeX (.tex) 序列化器
│       ├── parse.rs        # LaTeX → AST (有限子集 + raw 透传)
│       ├── citation.rs     # CitationResolver — 引文键 ↔ bib-base Article
│       ├── bibliography.rs # .bib 生成器 (复用 bib_base::export)
│       ├── compile.rs      # LatexEngine trait + Tectonic / XeLaTeX 后端
│       ├── template.rs     # 模板管理 (\documentclass, preamble, 预设)
│       ├── diff.rs         # AST diff / patch (block 级)
│       └── tools/          # Agent 工具
│           ├── mod.rs      # writing_all_registrations()
│           ├── document_tools.rs   # create/open/edit/outline
│           ├── citation_tools.rs   # resolve/insert/check
│           ├── compile_tools.rs    # compile/status
│           └── insert_tools.rs     # figure/table/data
│
├── node-bundles/
│   └── nodes-writing/      # DAG 节点 bundle
│       └── src/
│           ├── lib.rs      # Plugin
│           ├── table_node.rs       # DataFrame → LaTeX tabular
│           ├── figure_node.rs      # 产物路径 → \includegraphics
│           ├── render_node.rs      # Document AST → .tex + PDF
│           └── bib_compile_node.rs # 引文扫描 → .bib 文件
```

### Workspace 集成

```toml
# Cargo.toml (workspace)
[workspace.dependencies]
writing-types = { path = "crates/writing-types" }
writing-base  = { path = "crates/writing-base" }
nodes-writing = { path = "crates/node-bundles/nodes-writing", optional = true }

# data-engine/Cargo.toml
[features]
bundle-writing = ["dep:nodes-writing"]

# default_registry.rs
#[cfg(feature = "bundle-writing")]
registry.register_plugin(&nodes_writing::Plugin);
```

---

## 3. 文档模型 (`writing-types`)

### 3.1 核心数据结构

```rust
/// 一个写作项目 = 一篇论文 / 一份报告 / 一个章节。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Document {
    pub id: String,                    // UUID
    pub title: String,
    pub authors: Vec<DocumentAuthor>,  // 论文作者（≠文献作者）
    pub document_class: DocumentClass, // article | report | book | custom
    pub template_id: Option<String>,   // 关联模板（期刊/会议预设）
    pub root: Section,                 // 章节树的根
    pub preamble: Preamble,            // 自定义宏包/命令
    pub metadata: DocumentMetadata,    // 关键词、摘要、JEL codes 等
    pub version: u32,                  // 乐观锁版本号
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// 文档作者（含通讯作者标记、机构、ORCID）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DocumentAuthor {
    pub name: String,
    pub affiliation: Option<String>,
    pub orcid: Option<String>,
    pub corresponding: bool,
    pub email: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum DocumentClass {
    Article,
    Report,
    Book,
    Beamer,
    Custom(String),          // revtex4-2, elsarticle, etc.
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Preamble {
    pub packages: Vec<String>,         // \usepackage{...}
    pub macros: Vec<MacroDef>,         // \newcommand{...}
    pub custom: Vec<String>,           // 任意 preamble 行（逃生舱）
}
```

### 3.2 章节树

```rust
/// 章节：可递归嵌套。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Section {
    pub id: BlockId,                   // 稳定 ID，跨版本不变
    pub level: SectionLevel,           // Part/Chapter/Section/Subsection/...
    pub title: String,                 // 可含 Inline（通过 title_inlines 缓存）
    pub label: Option<String>,         // \label{...}
    pub children: Vec<Section>,        // 子章节
    pub blocks: Vec<Block>,            // 此节直属内容块
    pub status: SectionStatus,         // Draft/Revising/Final
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub enum SectionLevel {
    Part,       // \part
    Chapter,    // \chapter
    Section,    // \section
    Subsection, // \subsection
    Subsubsection,
    Paragraph,  // \paragraph
    Subparagraph,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub enum SectionStatus {
    Draft,
    Revising,
    Review,
    Final,
}
```

### 3.3 Block — 内容块

每个 block 有稳定 ID，是 Agent 编辑的最小粒度单位。

```rust
/// 内容块——文档的基本构件。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind")]
pub enum Block {
    Paragraph(ParagraphBlock),
    Equation(EquationBlock),
    Figure(FigureBlock),
    Table(TableBlock),
    CodeListing(CodeBlock),
    List(ListBlock),
    Quote(QuoteBlock),
    RawLatex(RawLatexBlock),   // 逃生舱：原样输出
    HorizontalRule,
    PageBreak,
}

/// 所有 block 的公共字段。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BlockMeta {
    pub id: BlockId,
    pub label: Option<String>,         // \label{...}
    pub tags: Vec<String>,             // 自由标签 (#methods, #results, ...)
    pub comments: Vec<BlockComment>,   // 批注（类似 Google Docs 评论）
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BlockComment {
    pub id: String,
    pub author: String,                // agent name 或 human
    pub text: String,
    pub resolved: bool,
    pub created_at: DateTime<Utc>,
}

pub type BlockId = String;             // UUID v4
```

### 3.4 Paragraph & Inline — 段落与行内内容

```rust
/// 段落：行内内容的有序序列。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ParagraphBlock {
    pub meta: BlockMeta,
    pub inlines: Vec<Inline>,
}

/// 行内内容。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum Inline {
    /// 纯文本。
    Text { content: String },
    /// 格式化文本。
    Formatted {
        content: String,
        format: TextFormat,            // Bold/Italic/Underline/Strikethrough/Monospace
    },
    /// 行内数学 `$...$`。
    InlineMath { latex: String },
    /// 引文簇（一组 cite key + 风格）。
    Citation(CitationCluster),
    /// 交叉引用 `\ref{label}` / `\cref{label}`。
    CrossRef {
        label: String,
        kind: RefKind,                 // Eq/Fig/Tab/Sec/Auto
        auto_prefix: bool,             // 是否使用 \cref (自动前缀)
    },
    /// 超链接。
    Link { url: String, text: String },
    /// 脚注。
    Footnote { content: Vec<Inline> },
    /// 行内声明的开始/结束标记（见 §3.5）。
    ClaimStart { claim_id: String },
    ClaimEnd { claim_id: String },
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub enum TextFormat { Bold, Italic, Underline, Strikethrough, Monospace, SmallCaps }

/// 引文簇——一个引用位置（可能同时引多篇）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CitationCluster {
    /// 引用的 cite key 列表（有序）。
    pub keys: Vec<CiteKey>,
    /// 引用命令风格。
    pub style: CitationStyle,
    /// 可选的前缀/后缀（如 "see [smith2024, p. 15]"）。
    pub prefix: Option<String>,
    pub suffix: Option<String>,
    /// 关联的声明 ID（如果此引文支撑某个 claim）。
    pub claim_id: Option<String>,
}

/// cite key——指向 bib-base Article 的引用键。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CiteKey {
    /// 键值，如 "smith2024breakthrough"。
    pub key: String,
    /// 如果已知，关联的 Article ID（bib-base 内部 ID）。
    pub article_id: Option<String>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub enum CitationStyle {
    /// \cite{key}
    Plain,
    /// \citep{key} — (Author, Year)
    Parenthetical,
    /// \citet{key} — Author (Year)
    Textual,
    /// \citeyear{key} — (Year)
    YearOnly,
    /// \citeauthor{key} — Author
    AuthorOnly,
    /// \footcite{key}
    Footnote,
}
```

### 3.5 Claim — 结构化声明标引

这是"结构化标引"的核心：在段落内标记声明（claim），每个声明挂载证据链。

```rust
/// 声明——文档中一段需要论证支持的话。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Claim {
    pub id: String,
    pub text_span: TextSpan,           // 在哪个 block 的哪个字符范围
    pub claim_type: ClaimType,
    pub evidence: Vec<EvidenceLink>,
    pub tags: Vec<String>,             // #hypothesis, #result, #limitation
    pub confidence: Option<Confidence>,
}

/// 声明类型。
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub enum ClaimType {
    Hypothesis,        // 假设
    Result,            // 结果
    Method,            // 方法描述
    Background,        // 背景陈述
    Limitation,        // 局限性
    FutureWork,        // 未来工作
    Definition,        // 定义
    Assumption,        // 假定
}

/// 证据链接——声明与支持材料之间的关联。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EvidenceLink {
    pub kind: EvidenceType,
    pub target: EvidenceTarget,
    pub note: Option<String>,          // 如何支撑此声明
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub enum EvidenceType {
    Citation,          // 引文支撑
    DataResult,        // 数据分析结果
    Figure,            // 图表
    Table,             // 表格
    ExternalUrl,       // 外部链接
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum EvidenceTarget {
    /// 指向 bib-base Article。
    Article { cite_key: String },
    /// 指向 DAG 节点输出（DataFrame artifact）。
    DagOutput {
        dag_id: String,
        node_id: String,
        port: String,
    },
    /// 指向文档内 block。
    BlockRef { block_id: BlockId },
    /// 外部 URL。
    Url(String),
}
```

### 3.6 Figure & Table

```rust
/// 图块。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FigureBlock {
    pub meta: BlockMeta,
    pub caption: Vec<Inline>,
    pub placement: Placement,          // h/t/b/p/H
    pub source: FigureSource,
    pub width: Option<Size>,           // \includegraphics[width=...]
    pub subfigures: Vec<SubfigureSpec>,// 非空时为子图
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum FigureSource {
    /// 文件路径（OpendalFileStorage 内）。
    FilePath(String),
    /// DAG artifact 产物路径。
    DagArtifact { dag_id: String, node_id: String },
    /// TikZ 代码（内联绘图）。
    Tikz(String),
}

/// 表格块。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TableBlock {
    pub meta: BlockMeta,
    pub caption: Vec<Inline>,
    pub placement: Placement,
    pub source: TableSource,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum TableSource {
    /// 显式单元格数据。
    Cells {
        header: Vec<String>,
        rows: Vec<Vec<TableCell>>,
        alignment: Vec<ColumnAlign>,   // l/c/r
    },
    /// DAG DataFrame 引用（编译时从节点输出拉取）。
    DagOutput {
        dag_id: String,
        node_id: String,
        port: String,
        columns: Option<Vec<String>>,  // None = 全部列
        max_rows: Option<usize>,       // 截断
        format: TableFormat,           // booktabs / plain / longtable
    },
    /// VFS SQL 查询。
    VFSQuery {
        sql: String,
        format: TableFormat,
    },
}
```

---

## 4. 引文管理 (`writing-base::citation`)

### 4.1 CitationResolver — 核心引擎

```rust
/// 引文解析器：桥接文档 cite key 与 bib-base Article 库。
pub struct CitationResolver {
    bib: Arc<BibBase>,
}

pub struct CitationReport {
    /// 文档中出现的所有 cite key。
    pub keys: BTreeSet<String>,
    /// 每个键的解析状态。
    pub statuses: Vec<CiteKeyStatus>,
    /// bib-base 中有但文档中未引用的 Article（可能遗漏）。
    pub uncited_in_collections: Vec<UncitedArticle>,
}

pub enum CiteKeyStatus {
    /// 成功匹配到 Article。
    Resolved { key: String, article_id: String },
    /// cite key 无法在 bib-base 中找到匹配。
    Unresolved { key: String, suggestion: Option<String> },
    /// 匹配到多篇（歧义）。
    Ambiguous { key: String, candidates: Vec<String> },
}

impl CitationResolver {
    /// 扫描整个文档，解析所有引文。
    pub fn scan(&self, doc: &Document) -> CitationReport { ... }

    /// 根据 cite key 查找 Article。
    pub fn resolve_key(&self, key: &str) -> Option<Article> { ... }

    /// 为文档中引用的所有 Article 生成 .bib 文件。
    /// 复用 bib_base::export::to_bibtex。
    pub fn generate_bib(&self, doc: &Document) -> Result<String> { ... }

    /// 引文图谱：block_id → 引用的 Article 列表。
    pub fn citation_graph(&self, doc: &Document) -> HashMap<BlockId, Vec<Article>> { ... }

    /// 一致性检查：删除 Article 时检查哪些文档受影响。
    pub fn check_deletion_impact(&self, article_id: &str, docs: &[Document])
        -> Vec<DeletionImpact> { ... }
}
```

### 4.2 引文键生成策略

复用 `bib_base::export::cite_key` 函数（`AuthorYearTitleWord` 格式），
增加冲突消解：

```rust
/// 生成无冲突的 cite key 集合。
pub fn generate_cite_keys(articles: &[Article]) -> HashMap<String, String> {
    // 1. 对每篇 Article 调用 bib_base::export::cite_key 生成基础键
    // 2. 检测冲突（相同键）
    // 3. 冲突时追加后缀字母: smith2024a, smith2024b, ...
    // 4. 返回 article_id → cite_key 映射
}
```

### 4.3 引文风格切换

```rust
/// 切换文档中所有引文的风格。
/// 例：Parenthetical → Textual，或切换到 biblatex 命令。
pub fn restyle_citations(doc: &mut Document, new_style: CitationStyle) { ... }

/// 切换引用包：natbib ↔ biblatex（涉及 preamble + 命令映射）。
pub fn switch_citation_package(
    doc: &mut Document,
    package: CitationPackage,
) -> Result<PreamblePatch> { ... }

pub enum CitationPackage {
    Natbib,   // \citep, \citet, \citeyear
    Biblatex, // \parencite, \textcite, \citeyear
    Apacite,  // APA 风格
}
```

---

## 5. LaTeX 序列化 (`writing-base::serialize`)

### 5.1 AST → .tex

```rust
/// 将 Document AST 序列化为完整的 .tex 文件内容。
pub fn render_document(doc: &Document, template: &Template) -> RenderedDocument {
    RenderedDocument {
        main_tex: render_main(doc, template),
        bib_content: ...,     // 如果用 inline bibliography
        files: vec![...],     // 辅助文件（如 .bib, 图片引用列表）
    }
}

pub struct RenderedDocument {
    pub main_tex: String,
    pub bib_content: Option<String>,
    pub required_files: Vec<FileRequirement>, // 编译所需文件清单
}
```

序列化器是一个 match 驱动的递归下降渲染器：

```rust
fn render_section(s: &Section, out: &mut String) {
    let cmd = match s.level {
        SectionLevel::Part => "\\part",
        SectionLevel::Section => "\\section",
        ...
    };
    writeln!(out, "{cmd}{{{s.title}}}");
    if let Some(label) = &s.label {
        writeln!(out, "\\label{{{label}}}");
    }
    for block in &s.blocks {
        render_block(block, out);
    }
    for child in &s.children {
        render_section(child, out);
    }
}

fn render_block(block: &Block, out: &mut String) {
    match block {
        Block::Paragraph(p) => {
            // ClaimStart/ClaimEnd 标记在序列化时转为 LaTeX 注释
            // 或自定义 \claimstart{id} / \claimend{id} 命令
            let text: String = p.inlines.iter().map(render_inline).collect();
            writeln!(out, "{text}\n");
        }
        Block::Figure(f) => {
            writeln!(out, "\\begin{{figure}}[{placement}]");
            // subfigures or \includegraphics
            writeln!(out, "\\caption{{{caption}}}");
            if let Some(label) = &f.meta.label {
                writeln!(out, "\\label{{{label}}}");
            }
            writeln!(out, "\\end{{figure}}\n");
        }
        Block::Table(t) => { /* tabular 环境 */ }
        Block::Equation(e) => {
            writeln!(out, "\\begin{{equation}}");
            writeln!(out, "{}", e.latex);
            writeln!(out, "\\end{{equation}}\n");
        }
        Block::RawLatex(r) => { out.push_str(&r.content); }
        ...
    }
}

fn render_inline(inline: &Inline) -> String {
    match inline {
        Inline::Text { content } => escape_latex(content),
        Inline::Formatted { content, format } => match format {
            TextFormat::Bold => format!("\\textbf{{{}}}", escape_latex(content)),
            TextFormat::Italic => format!("\\textit{{{}}}", escape_latex(content)),
            ...
        },
        Inline::Citation(cluster) => render_citation(cluster),
        Inline::InlineMath { latex } => format!("${latex}$"),
        Inline::CrossRef { label, auto_prefix, .. } => {
            if *auto_prefix { format!("\\cref{{{label}}}") }
            else { format!("\\ref{{{label}}}") }
        },
        ...
    }
}
```

### 5.2 LaTeX → AST（有限解析）

```rust
/// 将 .tex 文件解析回 AST。
///
/// 支持的子集：
/// - \section{...}, \subsection{...} 等层级命令
/// - \cite/\citep/\citet{...} 引文命令
/// - \ref/\cref{...} 交叉引用
/// - \begin{figure}/\end{figure}, \begin{table}/\end{table}
/// - \includegraphics[...]{...}
/// - \begin{equation}/\end{equation}
/// - $...$ 行内数学, $$...$$ display 数学
/// - \textbf{...}, \textit{...} 格式化
/// - \label{...}
///
/// 不支持的命令 → RawLatex 块透传。
pub fn parse_latex(tex: &str) -> Result<Document> { ... }
```

解析器使用 `nom` 或手写递归下降。策略是"宽容解析"：
- 认识的结构化命令 → 提升为 AST 节点
- 不认识的 → `RawLatex` 块原样保留
- 这保证任何 LaTeX 文件都能 round-trip

---

## 6. 模板系统 (`writing-base::template`)

```rust
/// 文档模板——预定义 \documentclass + preamble + 宏包组合。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Template {
    pub id: String,
    pub name: String,                  // "Nature Genetics", "IEEE Transactions"
    pub document_class: String,        // "article", "revtex4-2", "elsarticle"
    pub class_options: Vec<String>,    // [12pt, a4paper, twocolumn]
    pub preamble: String,              // 完整 preamble 文本
    pub citation_package: CitationPackage,
    pub bibliography_style: String,    // plainnat, unsrtnat, biblatex-gb7714
    pub builtin: bool,                 // 系统内置 vs 用户自定义
}

/// 内置模板注册表。
pub fn builtin_templates() -> Vec<Template> {
    vec![
        // 通用 article (natbib)
        Template { id: "generic-article", ... },
        // RevTeX (AJP, PRL, PRD 等 APS 期刊)
        Template { id: "revtex4-2", ... },
        // Elsevier (elsarticle)
        Template { id: "elsarticle", ... },
        // IEEE
        Template { id: "ieee", ... },
        // Springer Nature
        Template { id: "sn-article", ... },
        // 中华系列 (ctexart)
        Template { id: "ctexart", ... },
        // 学位论文 (基于 ctexbook)
        Template { id: "thesis", ... },
        // Beamer 幻灯片
        Template { id: "beamer", ... },
    ]
}
```

模板存储在 `writing-base` 的 Turso 库中，支持 CRUD。

---

## 7. 编译管道 (`writing-base::compile`)

### 7.1 LatexEngine trait

```rust
/// LaTeX 编译引擎——可插拔后端。
#[async_trait]
pub trait LatexEngine: Send + Sync {
    async fn compile(&self, input: &CompileInput) -> Result<CompileOutput>;
    fn name(&self) -> &'static str;
    fn is_available(&self) -> bool;
}

pub struct CompileInput {
    pub main_tex: String,              // 主文件内容
    pub bib_content: Option<String>,   // .bib 内容
    pub auxiliary_files: Vec<(String, String)>, // (filename, content)
    pub working_dir: Option<PathBuf>,  // None = 临时目录
}

pub struct CompileOutput {
    pub success: bool,
    pub pdf_bytes: Option<Vec<u8>>,
    pub log: String,                   // .log 文件内容
    pub warnings: Vec<CompileIssue>,
    pub errors: Vec<CompileIssue>,
    pub pages: Option<usize>,          // 页数（如果能解析）
    pub output_files: Vec<(String, Vec<u8>)>, // 所有输出文件
}

pub struct CompileIssue {
    pub line: Option<usize>,
    pub file: Option<String>,
    pub message: String,
    pub severity: IssueSeverity,
}
```

### 7.2 后端实现

```rust
/// Tectonic 后端——Rust 原生 TeX 引擎，可嵌入。
/// 优势：零外部依赖，自动下载宏包，单次编译。
pub struct TectonicEngine {
    // tectonic crate 作为依赖
}

/// XeLaTeX 子进程后端——需要系统安装 TeX Live / MiKTeX。
/// 优势：最大兼容性，支持 CJK (xeCJK), 所有宏包。
pub struct XelatexEngine {
    binary: PathBuf,
    // 需要 bibtex/biber 的路径
}

/// 自动选择策略：优先 Tectonic，fallback 到 XeLaTeX。
pub fn default_engine() -> Box<dyn LatexEngine> {
    if XelatexEngine::system_available() {
        Box::new(XelatexEngine::detect())
    } else {
        Box::new(TectonicEngine::new())
    }
}
```

**设计决策**：CJK 支持（中文写作）需要 XeLaTeX + xeCJK，Tectonic 对 CJK 的支持有限。
策略是：模板的 `requires_xelatex` 标记决定使用哪个引擎。

### 7.3 编译流水线

```rust
/// 完整的编译流水线。
pub async fn compile_document(
    doc: &Document,
    resolver: &CitationResolver,
    template: &Template,
    engine: &dyn LatexEngine,
) -> Result<CompileOutput> {
    // 1. AST → .tex
    let rendered = render_document(doc, template);

    // 2. 引文扫描 → .bib
    let bib = resolver.generate_bib(doc)?;

    // 3. 模板 + preamble 组装
    let main_tex = assemble_main_tex(rendered.main_tex, template, &doc.preamble);

    // 4. 引用包命令注入
    let main_tex = inject_citation_package(main_tex, template);

    // 5. 编译
    let input = CompileInput {
        main_tex,
        bib_content: Some(bib),
        ..Default::default()
    };
    engine.compile(&input).await
}
```

---

## 8. 存储 (`writing-base::store`)

### 8.1 WritingStore — Turso 后端

```sql
-- writing.db schema

CREATE TABLE IF NOT EXISTS documents (
    id          TEXT PRIMARY KEY,
    title       TEXT NOT NULL,
    data        TEXT NOT NULL,           -- JSON 序列化的完整 Document AST
    version     INTEGER NOT NULL DEFAULT 1,
    created_at  TEXT NOT NULL,
    updated_at  TEXT NOT NULL
);

-- 版本快照（每次保存自动创建）
CREATE TABLE IF NOT EXISTS document_versions (
    id          TEXT PRIMARY KEY,
    document_id TEXT NOT NULL REFERENCES documents(id) ON DELETE CASCADE,
    version     INTEGER NOT NULL,
    data        TEXT NOT NULL,           -- 该版本的完整 AST 快照
    diff        TEXT,                    -- 与前一版本的 EditScript diff
    author      TEXT,                    -- agent name / human
    message     TEXT,                    -- commit message
    created_at  TEXT NOT NULL,
    UNIQUE(document_id, version)
);

-- 模板
CREATE TABLE IF NOT EXISTS templates (
    id          TEXT PRIMARY KEY,
    name        TEXT NOT NULL,
    data        TEXT NOT NULL,           -- JSON 序列化的 Template
    builtin     INTEGER DEFAULT 0,
    created_at  TEXT NOT NULL
);

-- 编译产物缓存
CREATE TABLE IF NOT EXISTS compile_cache (
    document_id TEXT NOT NULL,
    version     INTEGER NOT NULL,
    pdf_path    TEXT,                    -- OpendalFileStorage 路径
    success     INTEGER NOT NULL,
    log         TEXT,
    compiled_at TEXT NOT NULL,
    PRIMARY KEY (document_id, version)
);

-- 文档 ↔ Collection 关联（一篇论文引用了哪些 collection 的文章）
CREATE TABLE IF NOT EXISTS document_collections (
    document_id  TEXT NOT NULL REFERENCES documents(id) ON DELETE CASCADE,
    collection_id TEXT NOT NULL,
    PRIMARY KEY (document_id, collection_id)
);
```

### 8.2 WritingStore API

```rust
pub struct WritingStore {
    conn: Arc<Connection>,
}

impl WritingStore {
    pub fn open(db_path: &str) -> Result<Self> { ... }
    pub fn open_in_memory() -> Result<Self> { ... }

    // 文档 CRUD
    pub fn create_document(&self, title: &str, class: DocumentClass) -> Result<Document> { ... }
    pub fn get_document(&self, id: &str) -> Result<Document> { ... }
    pub fn save_document(&self, doc: &Document, message: &str) -> Result<()> { ... }
    pub fn list_documents(&self) -> Result<Vec<DocumentSummary>> { ... }
    pub fn delete_document(&self, id: &str) -> Result<()> { ... }

    // 版本管理
    pub fn list_versions(&self, doc_id: &str) -> Result<Vec<VersionSummary>> { ... }
    pub fn get_version(&self, doc_id: &str, version: u32) -> Result<Document> { ... }
    pub fn diff_versions(&self, doc_id: &str, v1: u32, v2: u32) -> Result<EditScript> { ... }

    // 模板
    pub fn list_templates(&self) -> Result<Vec<Template>> { ... }
    pub fn get_template(&self, id: &str) -> Result<Template> { ... }
    pub fn upsert_template(&self, template: &Template) -> Result<()> { ... }

    // 编译缓存
    pub fn get_cached_pdf(&self, doc_id: &str, version: u32) -> Option<PathBuf> { ... }
    pub fn cache_compile_result(&self, ...) -> Result<()> { ... }
}
```

---

## 9. Agent 编辑操作 (`writing-base::tools`)

### 9.1 设计理念

Agent 编辑系统的核心设计原则：

1. **语义操作优先**：Agent 说"在 Methods 章节后插入 Results 章节"，而不是"在第 47 行后插入文本"
2. **稳定 ID 引用**：所有编辑操作通过 block_id / section_id 定位，不依赖行号
3. **可组合编辑脚本**：多个编辑操作打包成 `EditScript`，原子执行
4. **Dry-run 预览**：每个编辑操作可以先预览 diff 再 apply
5. **自动引文注入**：Agent 可以说"在这段话末尾引用 Smith 2024"，系统自动查找或提示保存

### 9.2 EditOp — 编辑操作语言

```rust
/// 原子编辑操作——所有 Agent 编辑命令最终分解为 EditOp 序列。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "action")]
pub enum EditOp {
    /// 插入章节。
    InsertSection {
        parent_id: Option<BlockId>,    // None = 根级别
        after_section: Option<BlockId>,// 插入位置
        section: Section,
    },
    /// 删除章节（含子内容）。
    DeleteSection {
        section_id: BlockId,
    },
    /// 移动章节（改变父级和/或顺序）。
    MoveSection {
        section_id: BlockId,
        new_parent: Option<BlockId>,
        after_section: Option<BlockId>,
    },
    /// 重命名章节。
    RenameSection {
        section_id: BlockId,
        new_title: String,
    },
    /// 插入内容块。
    InsertBlock {
        section_id: BlockId,
        after_block: Option<BlockId>,
        block: Block,
    },
    /// 替换内容块。
    ReplaceBlock {
        block_id: BlockId,
        new_block: Block,
    },
    /// 删除内容块。
    DeleteBlock {
        block_id: BlockId,
    },
    /// 移动内容块到另一个章节。
    MoveBlock {
        block_id: BlockId,
        to_section: BlockId,
        after_block: Option<BlockId>,
    },
    /// 追加段落文本（在现有段落末尾）。
    AppendToParagraph {
        block_id: BlockId,
        inlines: Vec<Inline>,
    },
    /// 替换段落内容。
    ReplaceParagraph {
        block_id: BlockId,
        inlines: Vec<Inline>,
    },
    /// 在指定位置插入引文。
    InsertCitation {
        block_id: BlockId,
        position: CitationPosition,    // End / AfterText(text) / BeforeText(text) / AtIndex(usize)
        keys: Vec<String>,
        style: CitationStyle,
    },
    /// 添加 claim 标记。
    TagClaim {
        block_id: BlockId,
        span: TextSpan,
        claim: Claim,
    },
    /// 更新元数据。
    UpdateMetadata {
        changes: MetadataChanges,
    },
    /// 修改 preamble（添加/删除宏包）。
    UpdatePreamble {
        changes: PreambleChanges,
    },
    /// 整体重排（传入新的章节大纲）。
    Restructure {
        new_outline: Outline,
    },
}

/// 编辑脚本——原子化的操作序列。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EditScript {
    pub ops: Vec<EditOp>,
    pub message: String,         // commit message
}
```

### 9.3 Agent 工具清单

#### 文档管理工具

| 工具 | 描述 |
|------|------|
| `doc_create` | 创建新文档（指定标题、文档类、模板） |
| `doc_open` | 打开文档，返回完整 AST 或指定章节 |
| `doc_outline` | 获取/修改文档大纲（章节树） |
| `doc_list` | 列出所有文档 |
| `doc_delete` | 删除文档 |
| `doc_metadata` | 查看/修改元数据（关键词、摘要等） |

#### 编辑工具

| 工具 | 描述 |
|------|------|
| `doc_insert_section` | 插入章节 |
| `doc_insert_block` | 插入内容块（段落/公式/图/表/列表） |
| `doc_replace_block` | 替换内容块 |
| `doc_delete_block` | 删除内容块 |
| `doc_move_block` | 移动内容块 |
| `doc_edit_script` | 提交复合编辑脚本（多操作原子执行） |
| `doc_raw_latex` | 插入原始 LaTeX（逃生舱） |

#### 引文工具

| 工具 | 描述 |
|------|------|
| `doc_add_citation` | 在指定位置插入引文（支持模糊查找 article） |
| `doc_check_citations` | 引文一致性检查（断引、歧义、遗漏） |
| `doc_citation_graph` | 查看引文图谱（block → articles） |
| `doc_restyle_citations` | 切换引文风格/包 |
| `doc_generate_bib` | 生成 .bib 文件预览 |

#### 结构化标引工具

| 工具 | 描述 |
|------|------|
| `doc_tag_claim` | 标记声明并挂载证据 |
| `doc_list_claims` | 列出文档中所有声明 |
| `doc_check_evidence` | 检查声明的证据完整性 |

#### 插入工具

| 工具 | 描述 |
|------|------|
| `doc_insert_table` | 插入表格（手动数据或 DAG 输出引用） |
| `doc_insert_figure` | 插入图片（文件路径或 DAG artifact） |
| `doc_insert_data` | 从 VFS 查询 → 表格/内联值 |
| `doc_cross_reference` | 创建交叉引用 |

#### 编译工具

| 工具 | 描述 |
|------|------|
| `doc_compile` | 编译文档为 PDF（异步，后台任务） |
| `doc_compile_status` | 查看编译进度/结果 |
| `doc_preview_tex` | 预览生成的 .tex 内容（不编译） |
| `doc_template_list` | 列出/管理模板 |

#### 版本工具

| 工具 | 描述 |
|------|------|
| `doc_history` | 查看版本历史 |
| `doc_diff` | 比较两个版本 |
| `doc_revert` | 回退到指定版本 |

### 9.4 典型工具实现示例

```rust
// doc_insert_block 工具
#[tool(
    name = "doc_insert_block",
    description = "Insert a content block (paragraph, equation, figure, table, ...) \
                  into a document section. Specify the section_id and optionally \
                  after_block to control position. For paragraphs, provide 'text' \
                  and optional inline citations. For equations, provide 'latex'. \
                  Returns the new block_id."
)]
pub struct DocInsertBlockInput {
    #[desc = "Document ID"]
    pub document_id: String,
    #[desc = "Target section ID to insert into"]
    pub section_id: String,
    #[desc = "Insert after this block ID. None = append to end of section."]
    pub after_block: Option<String>,
    #[desc = "Block type: paragraph | equation | figure | table | list | quote | code | raw"]
    pub block_type: String,
    #[desc = "Block content as structured JSON (schema depends on block_type)"]
    pub content: serde_json::Value,
    #[desc = "Optional tags for this block"]
    pub tags: Option<Vec<String>>,
}

pub struct DocInsertBlockTool {
    pub store: Arc<WritingStore>,
}

#[async_trait]
impl ToolFunction for DocInsertBlockTool {
    type Input = DocInsertBlockInput;
    async fn run(&self, i: Self::Input) -> Result<ToolResult, ToolError> {
        let mut doc = self.store.get_document(&i.document_id)?;
        let block = build_block_from_json(&i.block_type, &i.content, i.tags)?;
        let block_id = block.id().to_string();

        let edit = EditOp::InsertBlock {
            section_id: i.section_id,
            after_block: i.after_block,
            block,
        };
        apply_edit(&mut doc, &edit)?;
        self.store.save_document(&doc, "insert_block")?;

        Ok(ToolResult::success("block_id", &block_id))
    }
}
```

```rust
// doc_add_citation 工具
#[tool(
    name = "doc_add_citation",
    description = "Add a citation to a paragraph in the document. \
                  Automatically resolves cite keys against the local bibliography \
                  library. If the article is not yet in the library, the tool \
                  returns a suggestion to bib_save it first. \
                  \
                  Examples: \
                  • block_id='blk_abc', keys=['smith2024'], style='parenthetical' \
                  • block_id='blk_abc', position='end', keys=['smith2024','jones2023']"
)]
pub struct DocAddCitationInput {
    #[desc = "Document ID"]
    pub document_id: String,
    #[desc = "Block ID of the paragraph to add citation to"]
    pub block_id: String,
    #[desc = "Where to insert: 'end' | 'after:<text>' | 'before:<text>' | index number"]
    pub position: Option<String>,
    #[desc = "Cite keys or article IDs to cite"]
    pub keys: Vec<String>,
    #[desc = "Citation style: plain | parenthetical | textual | year_only | footnote"]
    pub style: Option<String>,
}

pub struct DocAddCitationTool {
    pub store: Arc<WritingStore>,
    pub bib: Arc<BibBase>,
}

#[async_trait]
impl ToolFunction for DocAddCitationTool {
    type Input = DocAddCitationInput;
    async fn run(&self, i: Self::Input) -> Result<ToolResult, ToolError> {
        let resolver = CitationResolver::new(self.bib.clone());
        let mut doc = self.store.get_document(&i.document_id)?;

        // 1. 解析 keys——尝试 cite_key 匹配，也尝试 article_id 匹配
        let mut resolved_keys = Vec::new();
        let mut unresolved = Vec::new();
        for key in &i.keys {
            if let Some(article) = resolver.resolve_key(key) {
                resolved_keys.push(CiteKey {
                    key: key.clone(),
                    article_id: Some(article.id),
                });
            } else {
                // 尝试作为 article_id 查找
                if let Ok(article) = self.bib.get_article(key) {
                    let ck = cite_key(&article);
                    resolved_keys.push(CiteKey {
                        key: ck,
                        article_id: Some(article.id),
                    });
                } else {
                    unresolved.push(key.clone());
                }
            }
        }

        if !unresolved.is_empty() {
            return Ok(ToolResult::warning(
                "unresolved_citations",
                format!("Cannot find these cite keys in the library: {unresolved:?}. \
                        Use bib_save to add them first."),
            ));
        }

        // 2. 执行编辑
        let edit = EditOp::InsertCitation {
            block_id: i.block_id,
            position: parse_position(i.position),
            keys: resolved_keys.iter().map(|k| k.key.clone()).collect(),
            style: parse_style(i.style),
        };
        apply_edit(&mut doc, &edit)?;
        self.store.save_document(&doc, "add_citation")?;

        Ok(ToolResult::success("added", &resolved_keys.len()))
    }
}
```

---

## 10. DAG 节点 (`nodes-writing`)

### 10.1 TableFromDfNode — DataFrame → LaTeX 表格

```rust
/// 将上游 DataFrame 转换为 LaTeX tabular 块。
/// 输出格式为 JSON（可直接被 doc_insert_table 消费）。
pub struct TableFromDfNodeSpec {
    pub format: TableFormat,           // Booktabs | Plain | Longtable
    pub caption: Option<String>,
    pub label: Option<String>,
    pub max_rows: Option<usize>,       // 截断行数
    pub max_cols: Option<usize>,
    pub float_placement: Option<String>,
    pub significance_stars: Option<bool>, // p 值转星号
}

pub struct TableFromDfNode { ... }

#[async_trait]
impl DagNode for TableFromDfNode {
    async fn execute(&self, ctx: &NodeCtx, inputs: &[NodeInput], ...) -> Result<PortOutputs, DagError> {
        let df = &inputs[0].data;
        // 1. 从 DataFrame 提取 schema + rows
        let (header, rows) = dataframe_to_rows(df, self.spec.max_rows)?;
        // 2. 生成 LaTeX tabular 文本
        let tex = render_tabular(&header, &rows, &self.spec)?;
        // 3. 输出为单行单列 DataFrame（content 列）
        Ok(PortOutputs::from([("default", make_string_df(tex)?)]))
    }
}
```

### 10.2 FigureEmbedNode — 产物路径 → LaTeX figure

```rust
/// 将一个图片文件路径包装为 LaTeX figure 块 JSON。
pub struct FigureEmbedNodeSpec {
    pub caption: Option<String>,
    pub label: Option<String>,
    pub width: Option<String>,          // "0.8\\textwidth"
    pub placement: Option<String>,
}

impl DagNode for FigureEmbedNode {
    async fn execute(...) -> Result<PortOutputs, DagError> {
        // 输入 port_0 = 包含 "path" 列的 DataFrame（通常来自可视化 artifact）
        let path = extract_path(&inputs[0].data)?;
        let figure_json = serde_json::json!({
            "source": { "FilePath": path },
            "caption": self.spec.caption,
            ...
        });
        Ok(PortOutputs::from([("default", make_json_df(figure_json)?)]))
    }
}
```

### 10.3 RenderDocumentNode — Document → PDF

```rust
/// 将 WritingStore 中的文档编译为 PDF。
/// 输入：document_id（spec 字符串）
/// 输出：PDF 文件路径（DataFrame）
pub struct RenderDocumentNodeSpec {
    pub document_id: String,
    pub template_id: Option<String>,   // 覆盖文档默认模板
}

impl DagNode for RenderDocumentNode {
    async fn execute(...) -> Result<PortOutputs, DagError> {
        // 1. 从 WritingStore 加载文档
        let doc = store.get_document(&self.spec.document_id)?;
        // 2. 引文解析 + .bib 生成
        let resolver = CitationResolver::new(bib);
        // 3. 编译
        let output = compile_document(&doc, &resolver, &template, engine.as_ref()).await?;
        // 4. 存 PDF 到 OpendalFileStorage
        let pdf_path = save_pdf(output.pdf_bytes)?;
        // 5. 返回路径
        Ok(PortOutputs::from([("default", make_string_df(&pdf_path)?)]))
    }
}
```

### 10.4 BibCompileNode — 引文扫描 → .bib

```rust
/// 扫描文档的所有引文，生成 .bib 文件内容。
/// 输出 port：bib_content (TEXT), citation_report (JSON)
pub struct BibCompileNodeSpec {
    pub document_id: String,
}
```

---

## 11. 系统集成

### 11.1 WritingShared — 进程级共享

```rust
/// 类比 BibShared，进程级打开一次、Arc 共享。
#[derive(Clone)]
pub struct WritingShared {
    pub store: Arc<WritingStore>,
    pub bib: Arc<bib_base::BibShared>,   // 复用已有的文献共享实例
    pub engine: Arc<dyn LatexEngine>,
    pub file_storage: Arc<OpendalFileStorage>,
}

impl WritingShared {
    pub fn open(config: &RuntimeConfig, bib: Arc<BibShared>) -> Result<Self> {
        let store = WritingStore::open(&config.writing_db_path())?;
        let engine = default_engine();
        Ok(Self {
            store: Arc::new(store),
            bib,
            engine: Arc::from(engine),
            file_storage: ...,
        })
    }
}
```

### 11.2 RuntimeConfig 扩展

```rust
// crates/runtime/src/config.rs

impl RuntimeConfig {
    /// 写作系统数据库路径。
    /// 环境变量: AUTONOMICS_WRITING_DB
    /// 默认: {state_dir}/writing.db
    pub fn writing_db_path(&self) -> PathBuf { ... }

    /// 是否启用写作系统。
    /// 环境变量: AUTONOMICS_ENABLE_WRITING=1
    /// 默认: true (当写作 crate 编译进来时)
    pub fn enable_writing(&self) -> bool { ... }
}
```

### 11.3 SharedInfra 扩展

```rust
// crates/runtime/src/host.rs — SharedInfra 增加字段
pub struct SharedInfra {
    // ... 现有字段 ...
    /// 写作系统共享实例。
    pub writing: Option<Arc<WritingShared>>,
}
```

### 11.4 AgentProfile 扩展

```rust
// AgentProfile 增加标志
pub struct AgentProfile {
    // ... 现有字段 ...
    /// 是否启用写作工具集。
    pub enable_writing: bool,
}
```

### 11.5 工具注册

```rust
// crates/runtime/src/tools.rs
pub fn writing_tools(writing: &WritingShared) -> Vec<ToolRegistration> {
    writing_base::tools::writing_all_registrations(
        writing.store.clone(),
        writing.bib.bib.clone(),
        writing.engine.clone(),
        writing.file_storage.clone(),
    )
}

// 在 tool_set_from_config 中：
if config.enable_writing() {
    let writing = WritingShared::open(config, infra.bib.clone())?;
    tools.extend(writing_tools(&writing));
}
```

---

## 12. 数据流总览

```
┌─────────────────────────────────────────────────────────────────────────────┐
│                              Agent (LLM)                                    │
│                                                                              │
│  "在 Results 第2段末尾引用 Smith 2024，                                     │
│   并把 MR 分析结果的 DataFrame 插入为 Table 3"                               │
└──────────┬──────────────────────────────────────────────────────────────────┘
           │ Agent 工具调用
           ▼
┌──────────────────────┐    ┌─────────────────────────────────────────────────┐
│  doc_add_citation    │    │  doc_insert_table                               │
│                      │    │                                                 │
│  1. 查找 cite key    │    │  1. 从 DAG 输出拉取 DataFrame                   │
│     (bib-base)       │    │  2. 转 LaTeX tabular                            │
│  2. EditOp::Insert   │    │  3. EditOp::InsertBlock                         │
│     Citation         │    │                                                 │
│  3. apply + save     │    │                                                 │
└──────────┬───────────┘    └──────────────┬──────────────────────────────────┘
           │                               │
           ▼                               ▼
┌─────────────────────────────────────────────────────────────────────────────┐
│                        Document AST (writing-base)                          │
│                                                                              │
│  Section "Results"                                                           │
│    └─ Paragraph (block_id=blk_42)                                           │
│         ├─ Text: "Our MR analysis shows..."                                 │
│         ├─ Citation: [smith2024]  ← CitationCluster                         │
│         └─ ClaimStart(id=claim_7)                                           │
│    └─ Table (block_id=blk_43)                                              │
│         └─ DagOutput { dag_id="mr_analysis", node_id="ivw" }                │
└──────────────────────────────┬──────────────────────────────────────────────┘
                               │ doc_compile
                               ▼
┌──────────────────────────────────────────────────────────────────────────────┐
│  编译流水线                                                                  │
│                                                                              │
│  1. AST → .tex (serialize.rs)                                               │
│  2. 引文扫描 → .bib (复用 bib_base::export::to_bibtex)                      │
│  3. 模板组装 → main.tex                                                      │
│  4. Tectonic/XeLaTeX 编译 → PDF                                             │
│  5. 缓存结果到 WritingStore                                                  │
└──────────────────────────────────────────────────────────────────────────────┘
```

---

## 13. 与 KMS 的集成

`Claim` 系统与 `kms`（知识管理系统）形成闭环：

```
阅读阶段：文献 → Annotation → KMS Entity/Knowledge
写作阶段：KMS Knowledge → Claim 标引 → 文档段落 → 引文回链
```

具体集成点：

```rust
/// 将文档中的声明同步到 KMS。
pub fn sync_claims_to_kms(
    doc: &Document,
    kms: &kms::Storage,
) -> Result<()> {
    for claim in doc.all_claims() {
        // 每个声明创建/更新一个 KMS Entity
        // 证据链（引文 + 数据）成为 Knowledge 节点
    }
}

/// 反向：从 KMS 知识图谱推荐引文。
pub fn recommend_citations_from_kms(
    doc: &Document,
    block_id: &BlockId,
    kms: &kms::Storage,
) -> Vec<CitationRecommendation> { ... }
```

---

## 14. 实现阶段

### Phase 0: 类型定义 + 存储 (`writing-types` + `writing-base::store`)
- `Document` / `Section` / `Block` / `Inline` 全部数据类型
- `WritingStore` Turso 后端 + schema migration
- `EditOp` / `EditScript` 定义
- AST 操作函数 (`ast.rs`)：apply_edit, find_block, walk_sections 等
- 基础测试

**验收标准**：能 CRUD 文档、apply 编辑脚本、round-trip AST ↔ JSON

### Phase 1: LaTeX 序列化 + 解析 (`serialize.rs` + `parse.rs`)
- AST → .tex 完整序列化器
- .tex → AST 有限解析器（支持常见命令 + raw 透传）
- 模板系统骨架 + 内置模板

**验收标准**：能将 AST 序译为可编译的 .tex 文件；已有 .tex 能导入为 AST

### Phase 2: 引文管理 (`citation.rs` + `bibliography.rs`)
- `CitationResolver`：cite key ↔ bib-base Article 解析
- .bib 文件生成（复用 `bib_base::export`）
- 引文一致性检查
- 引文图谱

**验收标准**：文档引文与 bib-base 完全联动

### Phase 3: 编译管道 (`compile.rs`)
- `LatexEngine` trait
- Tectonic 后端实现
- XeLaTeX 子进程后端实现
- 编译错误解析（.log → CompileIssue）
- 编译缓存

**验收标准**：AST → 完整 PDF，含引文和图表

### Phase 4: Agent 工具 (`tools/`)
- 文档管理工具（6 个）
- 编辑工具（7 个）
- 引文工具（5 个）
- 标引工具（3 个）
- 插入工具（4 个）
- 编译工具（4 个）
- 版本工具（3 个）
- 集成到 `RuntimeConfig` / `SharedInfra` / `AgentProfile`

**验收标准**：Agent 能通过工具完成完整写作流程

### Phase 5: DAG 节点 (`nodes-writing`)
- `TableFromDfNode`
- `FigureEmbedNode`
- `RenderDocumentNode`
- `BibCompileNode`
- 注册到 `data-engine` bundle

**验收标准**：DAG 输出能直接嵌入文档

### Phase 6: 结构化标引 + KMS 集成
- Claim 系统 + 证据链
- KMS 同步
- 引文推荐

**验收标准**：阅读→写作知识闭环

### Phase 7: TUI 集成
- 文档列表视图
- 章节树浏览
- 编译状态面板
- PDF 预览（外部 viewer 调用）

---

## 15. 依赖清单

```toml
# writing-types
[dependencies]
serde = { version = "1", features = ["derive"] }
serde_json = "1"
chrono = { version = "0.4", features = ["serde"] }

# writing-base
[dependencies]
writing-types = { workspace = true }
bib-types = { workspace = true }
bib-base = { workspace = true }
agentik-core = { workspace = true }    # ToolFunction trait
agentik-proc = { workspace = true }     # #[tool] macro
agentik-sdk = { workspace = true }      # ToolResult, ToolError
turso = { workspace = true }
tokio = { workspace = true }
async-trait = { workspace = true }
thiserror = { workspace = true }

# 编译后端（可选 feature）
[features]
tectonic = ["dep:tectonic"]             # 内嵌 TeX 引擎
xelatex = []                            # 子进程调用系统 xelatex

[dependencies.tectonic]
version = "0.15"
optional = true

# nodes-writing
[dependencies]
dag-core = { workspace = true }
datafusion = { workspace = true }
writing-types = { workspace = true }
writing-base = { workspace = true }
async-trait = { workspace = true }
serde_json = { workspace = true }
```

---

## 16. 关键设计决策记录

### D1: AST-first 而非 text-first

**决策**：文档内部模型是结构化 AST，LaTeX 是序列化目标而非存储格式。

**理由**：Agent 编辑需要语义操作（"移动第三章到第二章后面"），text diff 对 LaTeX
极其脆弱。AST-first 让每次编辑都是结构化的、可验证的、可 diff 的。
LaTeX → AST 解析器用于导入已有文档，之后全部在 AST 层操作。

### D2: 复用 bib-base 而非重建引文库

**决策**：引文系统直接桥接 `bib-base::BibBase`，不创建独立的引文数据库。

**理由**：bib-base 已经有完整的 Article CRUD、去重、全文、collection 管理。
写作系统只需要一个 `CitationResolver` 做 cite_key ↔ Article 映射 + .bib 生成。

### D3: Tectonic 优先 + XeLaTeX 后备

**决策**：默认尝试 Tectonic（Rust 原生），CJK 场景自动切换到 XeLaTeX。

**理由**：Tectonic 可嵌入、零外部依赖、适合容器化部署。但 CJK（中文写作）
需要 xeCJK，Tectonic 对此支持不稳定。模板的 `requires_xelatex` 标记自动决定。

### D4: EditScript 原子执行

**决策**：Agent 可以提交多个 EditOp 组成的 EditScript，系统原子执行。

**理由**：复杂编辑（如"重构整个 Results 章节"）需要多步骤但必须原子完成，
不能停在中间状态。EditScript 整体 apply 或全部回滚。

### D5: Claim 系统而非自由标签

**决策**：结构化标引使用显式的 Claim + EvidenceLink 类型，而非简单的自由标签。

**理由**：学术写作的核心是"声明→证据"链路。显式建模 Claim 让 Agent 能：
- 检查每个声明是否有引文支撑
- 检查证据完整性（数据引用是否指向有效 DAG 输出）
- 自动生成"声明摘要"（所有 hypothesis 是否被 result 验证）
- 与 KMS 形成知识闭环

### D6: DAG 节点输出 JSON 而非直接修改文档

**决策**：`nodes-writing` 的节点输出结构化 JSON（表格/图片块定义），
由 Agent 通过 `doc_insert_table` / `doc_insert_figure` 工具插入文档，
而非节点直接修改 WritingStore。

**理由**：保持 DAG 的"纯计算"语义（输入→输出 DataFrame）。文档修改是 Agent 的
职责。节点只负责格式转换，Agent 负责编排（决定插入位置、顺序、上下文）。
这也让节点输出可以被多次使用、预览、比较。

---

## 附录 A: 依赖图

```
writing-types ──────────────────────────────────────┐
    │                                                │
    ▼                                                ▼
writing-base ────────► bib-base ──────► bib-types   │
    │                   │                             │
    │                   ├──► eutils (PubMed SDK)     │
    │                   ├──► arxiv                    │
    │                   ├──► biorxiv                  │
    │                   └──► europepmc               │
    │                                                │
    ├──► agentik-core (ToolFunction)                 │
    ├──► agentik-proc (#[tool])                      │
    ├──► kms (optional, Phase 6)                    │
    └──► tectonic (optional, compile backend)       │
                                                     │
nodes-writing ──► dag-core ─────────────────────────┘
    │             │
    │             ├──► datafusion
    │             └──► arrow
    └──► writing-base
```
