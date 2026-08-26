# DAG 节点插件化架构设计

> 状态: 草案 · 2026-08-07

## 1. 现状与痛点

### 1.1 数据概览

| 维度 | 数量 |
|------|------|
| `nodes/` 下文件 | 65 个 `.rs` |
| 节点代码总量 | ~38,400 行 |
| 非节点基础设施 (dag/runtime/codegen/engine) | ~9,800 行 |
| `NodeRegistry::new()` 硬编码注册 | ~100 个 `register(Box::new(...))` |
| `fixture_spec()` 测试 | ~800 行 match arm |
| bio/stat 算法 crate 依赖 | ~20 个 |
| 已有的子目录模块 | `nodes/ml/` (14文件), `nodes/hypothesize/` (8文件) |

### 1.2 核心痛点

1. **编译时间** — 改任何一行节点代码，整个 38K LOC crate 全量重编译
2. **依赖耦合** — 只用 MR 分析也必须引入 R/ggplot2 (viz)、candle-core (ml)、magma 等全部依赖；无法做最小化部署
3. **无法扩展** — 第三方/社区想加节点必须 fork `data-engine`；没有插件接口
4. **心智模型** — 65 个扁平文件散落在 `nodes/` 下，无领域边界
5. **测试臃肿** — `registry.rs` 的 `fixture_spec()` 已 800+ 行，每加一个节点就要加一个 match arm

### 1.3 有利因素

- `ml/` 和 `hypothesize/` 已经用 `register_all(&mut registry)` 模式做了子目录拆分 — 这是天然的模板
- 节点间跨域依赖极少（仅 `sink_file → source_file::normalize_path` 和 `hdl_l_scan → hdl_l`）
- 外部消费者 (`data-engine-tools`, `runtime`) **几乎不直接引用节点类型**，只用 DAG/runtime API（唯一例外: `add_node_tool.rs` 引用 `SqlNodeSpec`）

## 2. 目标

| # | 目标 | 衡量标准 |
|---|------|----------|
| G1 | 节点可按领域独立编译 | 只用 MR 时无需编译 ml/survey/viz |
| G2 | 第三方可扩展 | 新增节点不需修改 `dag-core` 或 `data-engine` 源码 |
| G3 | 零注册样板 | 插件 crate 被 Cargo feature 选中即自动注册，无需手写 `register(Box::new(...))` |
| G4 | 向后兼容 | `data-engine` 的公共 API (`DataEngine`, `DataEngineClient`, DAG 工具链) 签名不变 |
| G5 | 渐进迁移 | 可分批迁移，每批后 `cargo test` 全绿 |

## 3. 架构设计

### 3.1 三层结构

```
┌──────────────────────────────────────────────────────┐
│                   data-engine (聚合层)                  │
│  DataEngine builder + Cargo features 选插件 + DagHistory │
└──────────────┬───────────────────────────────┬─────────┘
               │                               │
     ┌─────────▼─────────┐          ┌──────────▼──────────┐
     │    dag-core       │          │   node bundles      │
     │ (trait+registry+  │◄──────── │  nodes-io           │
     │  dag+codegen)     │  depend  │  nodes-mr           │
     │                   │   on     │  nodes-ldsc         │
     └───────────────────┘          │  nodes-ml           │
                                    │  nodes-survey       │
                                    │  ... (14 bundles)   │
                                    └─────────────────────┘
```

### 3.2 新 crate: `dag-core`

把 `data-engine` 中与具体节点无关的基础设施提取为独立 crate `crates/dag-core/`。

**迁入内容:**

| 来源 (data-engine/src/) | → dag-core 路径 | 说明 |
|--------------------------|-----------------|------|
| `nodes/meta.rs` | `node.rs` | `DagNode` trait, `NodePorts`, `Port`, `NodeInput`, `NodeId`, `string_opt_values` |
| `nodes/numeric_util.rs` | `arrow_util.rs` | `extract_numeric_*`, `ColumnError`, `crosstab` 等共享工具 |
| `nodes/sink_common.rs` | `sink.rs` | `SinkMode` |
| `node_registry/registry.rs` | `registry.rs` | `NodeFactory` trait, `NodeRegistry`, `NodeCtx`, `NodeInfo` |
| `node_registry/spec_normalize.rs` | `registry/spec_normalize.rs` | LLM spec 修复 |
| `node_registry/error.rs` | `registry/error.rs` | registry 错误类型 |
| `dag/*` | `dag/*` | DAG 图引擎 + 调度器 (graph, runtime, history, node_event, error, utils) |
| `codegen/*` | `codegen/*` | R/Python 代码生成 (context, compiler, helpers, mod) |
| `error.rs`, `dataset.rs`, `types.rs` | 同名 | 引擎级错误/类型 |

**新增: `NodePlugin` trait**

```rust
// dag-core/src/plugin.rs

/// 一个可插拔的节点包。
///
/// 每个 node-bundle crate 导出一个 `NodePlugin` 实现，
/// data-engine 通过 Cargo feature 选择启用哪些 bundle。
pub trait NodePlugin: Send + Sync {
    /// 包的唯一标识 (e.g. "mr", "ldsc", "ml")
    fn name(&self) -> &'static str;

    /// 将此包的所有 NodeFactory 注册到 registry
    fn register(&self, registry: &mut NodeRegistry);
}
```

**新增: `register_plugin` 方法**

```rust
impl NodeRegistry {
    /// 注册一个 NodePlugin（调用其 `register` 方法）
    pub fn register_plugin(&mut self, plugin: &dyn NodePlugin) {
        plugin.register(self);
    }
}
```

### 3.3 Node Bundle Crates

按领域拆分为 15 个独立 crate，统一放在 `crates/node-bundles/` 下:

| crate | 节点 | 依赖的算法 crate | LOC (≈) |
|-------|------|-----------------|---------|
| **nodes-io** | source_file, sink_file, container_command, ldsc_h2_container | opentargets, container-runtime, vfs | ~2,100 |
| **nodes-opengwas** | source_opengwas_associations, source_opengwas_phewas, source_opengwas_gwasinfo, source_opengwas_gwasinfo_search, source_opengwas_variants_rsid, source_opengwas_variants_chrpos, source_opengwas_ld_clump, source_opengwas_tophits | opengwas | ~1,000 |
| **nodes-sql** | sql_node, echo_node | — | 700 |
| **nodes-regression** | linear_regression, logistic_regression, cox_regression, chi_square | statkit | 1,400 |
| **nodes-causal** | mediation, causal, cmest (6 variants) | statkit, epi | 2,200 |
| **nodes-survival** | survival, fine_gray, cuminc | cmprsk | 1,400 |
| **nodes-ldsc** | ldsc_rg, ldsc_sldsc, liability, lcv | ldsc, lcv | 3,900 |
| **nodes-genetics** | lava (6), hdl_l, hdl_l_scan, mtag, cpassoc, susie_rss, magma (4), univariate_mixer, bivariate_mixer | lava, hdl, mtag, cpassoc, susie, magma, mixer | 8,200 |
| **nodes-mr** | two_sample_mr, mrlap, mrpresso, mvmr | mr, mrlap, mrpresso, mvmr | 4,000 |
| **nodes-coloc** | coloc, bkmr | coloc, bkmr | 1,300 |
| **nodes-epi** | epi_rcs, epi_roc, epi_lasso, epi_wqs, evalue | epi, evalue | 1,900 |
| **nodes-lcmm** | hlme, hlme_predict, hlme_compare | lcmm | 2,100 |
| **nodes-survey** | survey_* (30 nodes) | survey | 6,000 |
| **nodes-hypothesize** | hypothesize.* (24 nodes) | hypothesize | *(已有子模块)* |
| **nodes-ml** | ml.* (50+ nodes) | ml | *(已有子模块)* |
| **nodes-viz** | viz | visualization | 360 |
| 合计 | | | ~38,400 |

每个 bundle crate 结构:
```
crates/node-bundles/nodes-mr/
├── Cargo.toml          # depends on dag-core + mr, mrlap, mrpresso, mvmr
├── src/
│   ├── lib.rs          # pub struct Plugin; impl NodePlugin for Plugin
│   ├── two_sample_mr.rs
│   ├── mrlap.rs
│   ├── mrpresso.rs
│   └── mvmr.rs
└── tests/
```

### 3.4 插件注册机制

**方案: `NodePlugin` trait + 显式 `register_plugin` 调用**

每个 bundle crate 导出:
```rust
// nodes-mr/src/lib.rs
use dag_core::{NodePlugin, NodeRegistry};

pub struct Plugin;
impl NodePlugin for Plugin {
    fn name(&self) -> &'static str { "mr" }
    fn register(&self, registry: &mut NodeRegistry) {
        registry.register(Box::new(TwoSampleMrNodeFactory));
        registry.register(Box::new(MrlapNodeFactory));
        registry.register(Box::new(MrpressoNodeFactory));
        registry.register(Box::new(MvmrNodeFactory));
    }
}

// 公开供 data-engine 导入的工厂类型
pub use two_sample_mr::{TwoSampleMrNode, TwoSampleMrNodeFactory, TwoSampleMrSpec};
pub use mrlap::{MrlapNode, MrlapNodeFactory, MrlapSpec};
// ...
```

`data-engine` 的 `NodeRegistry::new()` 变为:
```rust
pub fn new(...) -> Self {
    let mut registry = Self { node_ctx, nodes: Default::default() };

    // 核心 bundle (总是启用)
    nodes_sql::Plugin.register(&mut registry);
    nodes_io::Plugin.register(&mut registry);

    // 可选 bundle (由 Cargo feature 控制)
    #[cfg(feature = "bundle-regression")]
    nodes_regression::Plugin.register(&mut registry);
    #[cfg(feature = "bundle-causal")]
    nodes_causal::Plugin.register(&mut registry);
    #[cfg(feature = "bundle-ldsc")]
    nodes_ldsc::Plugin.register(&mut registry);
    #[cfg(feature = "bundle-mr")]
    nodes_mr::Plugin.register(&mut registry);
    // ... 每个 bundle 一个 cfg gate

    registry
}
```

**为什么不用 `inventory` 自动注册?**

`inventory::submit!` 在链接时收集，但与 Cargo optional deps 联动是隐式的 — 关闭 feature 后符号消失但没有编译报错提示。显式 `register_plugin` + `#[cfg]` 更清晰，且 `register_all` 模式已被 `ml/` 和 `hypothesize/` 验证。

### 3.5 Cargo Feature 设计

```toml
# data-engine/Cargo.toml
[features]
# 默认全量 (向后兼容)
default = ["all-bundles"]

# 元 feature
all-bundles = [
    "bundle-regression", "bundle-causal", "bundle-survival",
    "bundle-ldsc", "bundle-genetics", "bundle-mr", "bundle-coloc",
    "bundle-epi", "bundle-lcmm", "bundle-survey",
    "bundle-hypothesize", "bundle-ml", "bundle-viz",
]

# 核心 bundle — 不可选 (总是编译)
# nodes-io + nodes-sql 始终包含

# 可选 bundle
bundle-regression  = ["dep:nodes-regression"]
bundle-causal      = ["dep:nodes-causal"]
bundle-survival    = ["dep:nodes-survival"]
bundle-ldsc        = ["dep:nodes-ldsc"]
bundle-genetics    = ["dep:nodes-genetics"]
bundle-mr          = ["dep:nodes-mr"]
bundle-coloc       = ["dep:nodes-coloc"]
bundle-epi         = ["dep:nodes-epi"]
bundle-lcmm        = ["dep:nodes-lcmm"]
bundle-survey      = ["dep:nodes-survey"]
bundle-hypothesize = ["dep:nodes-hypothesize"]
bundle-ml          = ["dep:nodes-ml"]
bundle-viz         = ["dep:nodes-viz"]

[dependencies]
dag-core = { path = "../dag-core" }
nodes-io = { path = "node-bundles/nodes-io" }
nodes-sql = { path = "node-bundles/nodes-sql" }
nodes-regression = { path = "node-bundles/nodes-regression", optional = true }
# ... 每个 bundle
```

下游使用:
```toml
# 只需要 MR 分析 — 最小化依赖
data-engine = { path = "...", default-features = false, features = ["bundle-mr"] }

# 除了 ML 和 survey 之外全部启用
data-engine = { path = "...", default-features = false, features = [
    "bundle-regression", "bundle-causal", "bundle-survival",
    "bundle-ldsc", "bundle-genetics", "bundle-mr", "bundle-coloc",
    "bundle-epi", "bundle-lcmm", "bundle-hypothesize", "bundle-viz",
]}
```

### 3.6 fixture_spec 测试分发

当前 `registry.rs` 的 800 行 `fixture_spec` 是迁移最大障碍之一。方案: **每个 bundle crate 自带测试 fixture**。

```rust
// dag-core: registry 测试基础设施
pub trait NodePlugin {
    // ...
    /// 测试用的最小合法 spec (返回 None 则跳过该 bundle 的 build 测试)
    fn fixture_spec(&self, kind: &str) -> Option<serde_json::Value> { None }
}

// dag-core 中的通用测试 (macro 或函数):
pub fn assert_all_factories_build(registry: &NodeRegistry) {
    for info in &registry.list_nodes() {
        // 如果 fixture_spec 返回 None，跳过
        // 否则 build_node 并验证 kind 一致性 + ports 一致性
    }
}
```

每个 bundle crate 的 `impl NodePlugin` 中实现 `fixture_spec`，把当前 `registry.rs` 里的 match arm 分发到各 bundle。

### 3.7 跨节点依赖处理

发现的跨节点依赖:

| 依赖 | 处理 |
|------|------|
| `sink_file → source_file::normalize_path` | 提取到 `dag-core::arrow_util` 或 `nodes-io` 内部共享 |
| `hdl_l_scan → hdl_l::*` (节点间) | 同属 `nodes-genetics`，bundle 内 `pub use` 解决 |
| `nodes/ml/* → nodes/ml/common.rs` | 已在 bundle 内部，直接迁移 |
| `nodes/hypothesize/* → nodes/hypothesize/common.rs` | 同上 |
| `sink_file` 的路径规范化 helper | 随 `nodes-io` 迁移 |

## 4. 迁移计划

### Phase 0: 提取 `dag-core` (基础设施)

**目标**: 把 `data-engine` 拆为 `dag-core` (trait+引擎) + `data-engine` (壳) ，节点暂时全部留在 `data-engine` 中。

**步骤**:
1. 创建 `crates/dag-core/`，复制 `dag/`, `codegen/`, `node_registry/`, `nodes/meta.rs`, `nodes/numeric_util.rs`, `nodes/sink_common.rs`, `nodes/ldsc_common.rs`, `nodes/survey_common.rs`, `error.rs`, `dataset.rs`, `types.rs`
2. 定义 `NodePlugin` trait + `NodeRegistry::register_plugin`
3. `data-engine` 的 `Cargo.toml` 加 `dag-core` 依赖；`nodes/mod.rs` 改为 `pub use dag_core::{DagNode, NodeInput, ...}`
4. 所有节点的 `use super::meta::*` → `use dag_core::node::*`; `use super::numeric_util::*` → `use dag_core::arrow_util::*`
5. `NodeRegistry::new()` 保持在 `data-engine` 中，但注册调用暂时不变
6. `cargo test` 全绿

**验证点**: `data-engine` 公共 API 不变，所有外部消费者无需改动。

### Phase 1: 迁移已模块化的 bundle (`nodes-ml`, `nodes-hypothesize`)

**目标**: 验证 bundle crate 模式可行。

**步骤**:
1. 创建 `crates/node-bundles/nodes-ml/`，把 `nodes/ml/` 整个目录搬过去
2. `Cargo.toml` 依赖 `dag-core` + `ml` crate
3. 导出 `Plugin` 实现 `NodePlugin`
4. `data-engine` 加 `bundle-ml` feature
5. 同理处理 `nodes-hypothesize`
6. `cargo test` 全绿

### Phase 2: 迁移无复杂依赖的 bundle (6 个)

按依赖简单程度排序:

| 批次 | bundle | 原因 |
|------|--------|------|
| 2a | nodes-viz | 最小 (360 LOC, 1节点) |
| 2b | nodes-sql | 无算法依赖 (700 LOC) |
| 2c | nodes-regression | 只依赖 statkit (1400 LOC) |
| 2d | nodes-survival | 只依赖 cmprsk (1400 LOC) |
| 2e | nodes-coloc | coloc + bkmr (1300 LOC) |
| 2f | nodes-epi | epi + evalue (1900 LOC) |

每批: 创建 crate → 搬代码 → `Plugin` impl → data-engine feature gate → test

### Phase 3: 迁移中等复杂度 bundle (5 个)

| 批次 | bundle | 节点数 | 特殊注意 |
|------|--------|--------|----------|
| 3a | nodes-io | 3 | 提取 `normalize_path` 到共享位置 |
| 3b | nodes-causal | 8 | cmest 6 变体 |
| 3c | nodes-lcmm | 3 | hlme 3 节点 |
| 3d | nodes-mr | 4 | mr + mrlap + mrpresso + mvmr |
| 3e | nodes-survey | 30 | survey_common 大量共享代码 |

### Phase 4: 迁移高复杂度 bundle (2 个)

| 批次 | bundle | 节点数 | 特殊注意 |
|------|--------|--------|----------|
| 4a | nodes-ldsc | 5 | ldsc_common 共享; liability 节点 |
| 4b | nodes-genetics | 16 | 最大 bundle; hdl_l_scan → hdl_l 跨节点引用; mixer 两个节点 |

### Phase 5: 清理收尾

1. `data-engine/src/nodes/` 目录清空（仅保留 `mod.rs` 做 re-export shim 或完全删除）
2. `NodeRegistry::new()` 完全用 `register_plugin` 替代硬编码注册
3. `fixture_spec` 分发到各 bundle
4. 删除 `registry.rs` 中的 800 行 fixture_spec
5. 更新 `data-engine/src/lib.rs` 公共导出
6. 更新 `data_engine.rs` re-exports
7. 处理 `add_node_tool.rs` 中 `SqlNodeSpec` 的导入路径
8. 全量 `cargo test` + 更新 CLAUDE.md

## 5. 目录结构 (完成后)

```
crates/
├── dag-core/                      # 引擎核心 (trait + DAG + registry + codegen)
│   ├── src/
│   │   ├── lib.rs
│   │   ├── node.rs                # DagNode, NodePorts, Port, NodeInput
│   │   ├── plugin.rs              # NodePlugin trait
│   │   ├── arrow_util.rs          # extract_numeric_*, ColumnError, crosstab
│   │   ├── sink.rs                # SinkMode
│   │   ├── registry.rs            # NodeFactory, NodeRegistry, NodeCtx
│   │   ├── registry/
│   │   │   ├── spec_normalize.rs
│   │   │   └── error.rs
│   │   ├── dag/
│   │   │   ├── graph.rs
│   │   │   ├── runtime.rs
│   │   │   ├── history.rs
│   │   │   ├── node_event.rs
│   │   │   ├── error.rs
│   │   │   └── utils.rs
│   │   └── codegen/
│   │       ├── context.rs
│   │       ├── compiler.rs
│   │       └── helpers.rs
│   └── Cargo.toml
│
├── data-engine/                   # 聚合层 (thin shell)
│   ├── src/
│   │   ├── lib.rs
│   │   ├── data_engine.rs         # DataEngine builder + feature-gated plugin registration
│   │   ├── runtime.rs             # DataEngineClient / DataEngineManager
│   │   └── runtime/
│   │       ├── types.rs
│   │       └── error.rs
│   └── Cargo.toml                 # features: bundle-mr, bundle-ml, ...
│
├── node-bundles/
│   ├── nodes-io/
│   ├── nodes-sql/
│   ├── nodes-regression/
│   ├── nodes-causal/
│   ├── nodes-survival/
│   ├── nodes-ldsc/
│   ├── nodes-genetics/
│   ├── nodes-mr/
│   ├── nodes-coloc/
│   ├── nodes-epi/
│   ├── nodes-lcmm/
│   ├── nodes-survey/
│   ├── nodes-hypothesize/
│   ├── nodes-ml/
│   └── nodes-viz/
│
├── data-engine-tools/             # 不变 (只用 dag-core + data-engine runtime API)
├── runtime/                       # 不变
└── ... 其他 crate
```

## 6. 公共 API 兼容性

### 保持不变的路径

```rust
// 这些路径对外部消费者不变 (data-engine re-export)
data_engine::dag::DagNode           // → data_engine re-exports dag_core::dag::DagNode
data_engine::dag::DAG
data_engine::data_engine::DataEngine
data_engine::runtime::DataEngineClient
data_engine::codegen::CodegenTarget
```

### 需要调整的路径

```rust
// 之前
data_engine::nodes::sql_node::SqlNodeSpec

// 之后 (add_node_tool.rs 需更新)
dag_core::...  // 不对，SqlNodeSpec 在 nodes-sql 中
// 方案: data-engine re-export: pub use nodes_sql::SqlNodeSpec;
// 或: add_node_tool 直接依赖 nodes-sql
```

### data-engine 的 re-export shim

为了最大兼容，`data-engine/src/lib.rs` 保持 re-export:

```rust
pub use dag_core::{
    dag, codegen,
    node::{DagNode, NodeInput, NodePorts, Port, NodeId, DEFAULT_PORT},
    registry::{NodeFactory, NodeRegistry, NodeCtx, NodeInfo},
};

// 节点类型的 re-export (feature-gated)
#[cfg(feature = "bundle-sql")]
pub use nodes_sql::{SqlNode, SqlNodeFactory, SqlNodeSpec};

#[cfg(feature = "bundle-mr")]
pub use nodes_mr::{TwoSampleMrNode, TwoSampleMrNodeFactory, TwoSampleMrSpec};
// ...
```

## 7. 编译时间收益预估

| 场景 | 当前 | 之后 |
|------|------|------|
| 全量编译 (default features) | ~T (38K LOC 单线程) | ~T (但并行编译 14 个 crate) |
| 只编译 MR bundle | ~T (全部) | ~0.15T (mr + core + io + sql) |
| 改一个 ml 节点 | 重编译整个 data-engine | 只重编译 nodes-ml (14文件) |
| CI 增量编译 | 全量 | 增量 (只编译变更的 bundle + 依赖) |

并行编译 + 选择性编译预计可将增量编译时间降低 60-80%。

## 8. 风险与缓解

| 风险 | 影响 | 缓解 |
|------|------|------|
| 迁移过程中破坏测试 | 高 | 每个 Phase 结束时 `cargo test` 全绿；Phase 0 不移代码只加 indirection |
| 共享代码归属争议 | 中 | 原则: 被 ≥2 个 bundle 用 → 放 dag-core；只被 1 个 bundle 用 → 留在 bundle 内部 |
| Cargo feature 矩阵爆炸 | 中 | 不在 bundle crate 内部再做 feature；bundle 是最小编译单元 |
| 外部消费者路径断裂 | 低 | data-engine 保持 re-export shim；Phase 5 统一更新 |
| `codegen` 跨 bundle 引用节点类型 | 中 | codegen trait + NodePlugin 在 dag-core；bundle 各自实现 codegen_r/python |

## 9. 后续演进 (本设计范围外)

- **动态插件加载**: 通过 WASM (wasmtime) 或 C ABI 实现运行时 `.so`/`.wasm` 插件发现，社区贡献节点无需编译进主仓库
- **插件版本管理**: NodePlugin trait 加 `fn version(&self) -> semver::Version`
- **插件市场**: `data-engine.toml` 配置文件声明启用插件列表，运行时从 crates.io 或内部 registry 拉取
