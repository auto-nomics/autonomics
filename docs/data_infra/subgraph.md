# Tag-Induced LD Subgraph 数据基础设施

> **命名空间**：`iceberg.mixer`  
> **表**：`eur_subgraph`（全基因组合并为一张表）  
> **人群**：EUR（1000 Genomes Phase 3）  
> **构建状态**：已上线，267,215,170 行（2.67 亿条 LD 边）

## 数据结构

单张 Iceberg 表，每行是一条 tag 诱导的 LD 边：

| 列名      | 类型     | 含义                                    |
|-----------|----------|-----------------------------------------|
| `chrom`   | Int64    | 染色体号（1–22）                        |
| `id_a`    | Utf8     | SNP A 的 rsid                           |
| `id_b`    | Utf8     | SNP B 的 rsid                           |
| `r2`      | Float32  | 未相位 LD r²                            |
| `h_a`     | Float32  | SNP A 的杂合度 2·maf·(1-maf)           |
| `h_b`     | Float32  | SNP B 的杂合度                          |

**语义**：每行是参考面板里 r²≥0.05 的一对 SNP，且**至少一端是 tag**（tag 诱导）。
非 tag↔非 tag 的对不在表里——fold 不需要它们。

**rsid 标识**：用 rsid 字符串（非整数 index），因为 precompute 和消费节点（mixer
节点）的 index 空间不同（全 AF 面板 vs universe），整数 index 无法跨进程对齐。

**h 预算好**：杂合度在 precompute 时从 AF 算好存入，消费方不再查 AF 表取 h。

## 构建方式

三步管线：**逐染色体预算** → **合并 + 转 rsid** → **上传 Iceberg**。

```
af.eur_af + ld_matrix.eur_chr{N}
    │
    ▼  precompute_tags2 <N>  （per-chrom，逐条跑）
select_tags → tag 集合
    │
    ▼  筛 tag 诱导边（任一端是 tag, r²≥0.05）
chr{N}_subgraph.parquet  （UInt32 local index + h）
    │
    ▼  merge_subgraph  （读 AF 重建 rsid 列表，local idx → rsid）
eur_subgraph.parquet  （rsid + chrom + h，单文件）
    │
    ▼  lake_cli upload --parquet --overwrite
iceberg.mixer.eur_subgraph  （SQL 可查询）
```

### 第一步：逐染色体预算

**工具**：`cargo run -p data-engine --bin precompute_tags2 -- {N}`

对每条染色体：
1. 读 AF（`af.eur_af WHERE chrom=N`）→ rsids / maf_vec / rsid→idx / h_vec。
2. 读 LD 一次（`ld_matrix.eur_chr{N} WHERE r²≥0.05`）→ 物化 `Vec<(a,b,r²)>`。
3. 从物化数据筛 r²>0.8 建邻接 CSR → `select_tags` → tag 集合。
4. 从物化数据筛 tag 诱导边（任一端是 tag）→ `chr{N}_subgraph.parquet`。

subset 按染色体 SNP 占比分配（`GLOBAL_SUBSET × chr_snps / total_snps`）。

**逐条跑全基因组**：
```bash
for c in $(seq 1 22); do
    cargo run -p data-engine --bin precompute_tags2 -- $c
done
```

产物：`chr1_subgraph.parquet` … `chr22_subgraph.parquet`（per-chrom，UInt32
local index + h_a/h_b）。

### 第二步：合并 + 转 rsid

**工具**：`cargo run -p data-engine --bin merge_subgraph -- . eur_subgraph.parquet`

对每条染色体：
1. 读 AF 重建 rsid 列表（和 precompute_tags2 同样的扫描顺序）。
2. 读 `chr{N}_subgraph.parquet` 的 `a_idx/b_idx`（UInt32 local index）。
3. 查 `rsids[a_idx]` → rsid 字符串。
4. 输出 `(chrom, id_a, id_b, r2, h_a, h_b)`。

产物：`eur_subgraph.parquet`（单文件，rsid 格式，~2.3 GB）。

### 第三步：上传 Iceberg

```bash
cargo run -p data-engine --bin lake_cli -- \
    upload eur_subgraph.parquet mixer.eur_subgraph --parquet --overwrite
```

## 所需原料

| 原料                    | 来源                                      | 说明                          |
|-------------------------|-------------------------------------------|-------------------------------|
| `af.eur_af`             | Iceberg 数据湖（AF 基础设施）             | MAF / rsid 列表 / 杂合度      |
| `ld_matrix.eur_chr{N}`  | Iceberg 数据湖（LD matrix 基础设施）      | r² LD 对（precompute 扫描）   |
| `mixer::extract::select_tags` | bio_crates/mixer                     | tag 选择算法（MAF+subset+剪枝）|

## 关键参数

| 参数            | 默认值     | 位置                 | 含义                          |
|-----------------|------------|----------------------|-------------------------------|
| MAF_MIN         | 0.05       | precompute_tags2     | tag 候选 MAF 下限             |
| R2_PRUNE        | 0.8        | precompute_tags2     | LD 剪枝阈值（tag 选择用）     |
| R2_MIN          | 0.05       | precompute_tags2     | 子图最低 r²（fold 用）        |
| GLOBAL_SUBSET   | 2,000,000  | precompute_tags2     | 全基因组 subset 总预算        |
| SEED            | 123        | precompute_tags2     | 随机种子（可复现）            |

## 下游消费方

| 消费方               | 查询                                                  | 用途                      |
|----------------------|-------------------------------------------------------|---------------------------|
| univariate_mixer     | `SELECT ... FROM eur_subgraph WHERE chrom={N}`       | LD fold（替代 ld_matrix） |
| bivariate_mixer      | 同上                                                  | LD fold                   |
| 任何 SQL 工具        | `SELECT COUNT(*) FROM mixer.eur_subgraph`             | 数据探查                  |

## 验证

```bash
# 行数
cargo run -p data-engine --bin lake_cli -- count mixer.eur_subgraph
# → mixer.eur_subgraph: 267215170 行

# schema
cargo run -p data-engine --bin lake_cli -- schema mixer.eur_subgraph

# 抽样
cargo run -p data-engine --bin lake_cli -- query \
    "SELECT * FROM iceberg.mixer.eur_subgraph WHERE chrom = 22 LIMIT 10"
```

## 规模参考

| 指标         | 值             |
|--------------|----------------|
| 总行数       | 267,215,170    |
| 文件大小     | ~2.3 GB (Parquet) |
| 染色体数     | 22             |
| 最大染色体   | chr6 (35.6M 边) |
| 最小染色体   | chr21 (2.9M 边) |
| tag 总数     | ~2M（全基因组） |

## 重建条件

以下任一变化时需重新构建：

| 变化                  | 重建？ |
|-----------------------|--------|
| 换 GWAS 性状          | **否**（子图与 GWAS 无关） |
| 改 MAF/r²/subset/seed | **是** |
| 换参考面板 / AF / LD  | **是** |
| 加新染色体            | **是**（跑新染色体的 precompute + 重新 merge） |
