# LD Matrix 数据基础设施

[English](ld_matrix.md) | [中文](ld_matrix_zh.md)

> **命名空间**：`iceberg.ld_matrix`  
> **表**：`eur_chr1` … `eur_chr22`（一条染色体一张表）  
> **人群**：EUR（1000 Genomes Phase 3）  
> **构建状态**：已上线，chr1–22 全量入库

## 数据结构

每条染色体一张 Iceberg 表，schema 如下：

| 列名           | 类型      | 含义                              | 源 TSV 列头     |
|----------------|-----------|-----------------------------------|-----------------|
| `chrom_a`      | Int64     | SNP A 的染色体号                  | `#CHROM_A`      |
| `pos_a`        | Int64     | SNP A 的物理坐标（bp）            | `POS_A`         |
| `id_a`         | Utf8      | SNP A 的 rsid                     | `ID_A`          |
| `chrom_b`      | Int64     | SNP B 的染色体号                  | `#CHROM_B`      |
| `pos_b`        | Int64     | SNP B 的物理坐标（bp）            | `POS_B`         |
| `id_b`         | Utf8      | SNP B 的 rsid                     | `ID_B`          |
| `unphased_r2`  | Float64   | 未相位 LD 的 r²（0–1）           | `UNPHASED_R2`  |

**语义**：每行是一对 SNP 的 LD（r²）。只存 `r² ≥ LD_R2_MIN`（默认 0.01）的对——
稀疏存储，不存 r²≈0 的无关对。

**表内无整数 SNP ID**——SNP 用 rsid 字符串标识。整数 index 是消费方运行时临时
建的，不持久化（详见消费方文档）。

## 构建方式

完整管线分两段：**上游 PLINK2 计算** → **Iceberg 入库**。

```
1000G VCF (raw)
    │
    ▼  infra/thousand_genomes/ld_matrix.sh
PLINK2 QC + --r2 计算
    │
    ▼
zstd TSV (per-chrom sparse LD matrix)
    │
    ▼  infra/sink_ld_matrix (Rust binary)
iceberg.ld_matrix.eur_chr{N} (per-chrom table)
```

### 第一段：上游 PLINK2 计算

**脚本**：`infra/thousand_genomes/ld_matrix.sh`

```bash
./ld_matrix.sh EUR          # 跑 EUR 人群 chr1–22
POPS="EUR" CHRS="21 22" ./ld_matrix.sh   # 指定
```

**步骤**：
1. 从 1000G panel 文件提取 EUR 样本列表（FID/IID）。
2. 按染色体将 VCF 转为 PLINK bfile（`.bed/.bim/.fam`）。
3. QC 过滤：
   - `--maf 0.01`（MAF ≥ 1%，1000G N=503 的统计功效底线）
   - `--geno 0.05`（SNP 缺失率 ≤ 5%）
4. PLINK2 `--r2` 计算 LD：
   - 窗口 `10 Mb`（LD 衰减距离，超过此距离 r²≈0）
   - 阈值 `r² ≥ 0.01`（只存有意义的对）
5. 输出：每条染色体一个 `.ld.vcor.zst`（zstd 压缩 TSV）。

**输出目录**：`/mnt/disk2/dataset/1000g_plink/eur/ld/`

### 第二段：Iceberg 入库

**工具**：`infra/sink_ld_matrix`（`cargo run -p sink_ld_matrix`）

读取 zstd TSV，按 schema 列名**位置赋值**（`CsvReadOptions::schema`），写入
Iceberg 表。每条染色体独立写入，并发度 = CPU 核数（`SINK_CONCURRENCY=N` 可调）。

- **幂等**：已有 snapshot 的表跳过；被 kill 的空表下次自动补。
- **重写**：先 `drop_table` 再跑。

**列名坑**：源 TSV 首列 `#CHROM_A` 带 `#`，DataFusion 当成 qualifier 分隔符 →
name-based 重命名静默失败。所以用**位置 schema** 赋名，且 `#` 被 Iceberg 字段名
规范拒绝 → 统一转小写 `chrom_a`。

## 所需原料

| 原料                | 位置 / 来源                                          | 说明                          |
|---------------------|------------------------------------------------------|-------------------------------|
| 1000G VCF           | `/mnt/disk2/dataset/1000g_genotype_data/`            | 1000G Phase 3 全基因组分型    |
| 样本 panel          | `integrated_call_samples_v3.20130502.ALL.panel`      | 人群→样本映射（EUR/AFR/…）   |
| PLINK2              | 系统 PATH                                            | LD 计算（`--r2`）             |
| zstd                | 系统 PATH                                            | TSV 压缩                      |
| Iceberg REST catalog| 数据湖配置（`Datalake::new()`）                      | 表存储后端                    |

## 关键参数

| 参数          | 值      | 位置                 | 含义                                   |
|---------------|---------|----------------------|----------------------------------------|
| MAF_MIN       | 0.01    | ld_matrix.sh         | SNP 最低 MAF（QC 过滤）                |
| GENO_MAX      | 0.05    | ld_matrix.sh         | SNP 最大缺失率（QC 过滤）              |
| LD_WINDOW_KB  | 10000   | ld_matrix.sh         | LD 计算窗口（10 Mb）                   |
| LD_R2_MIN     | 0.01    | ld_matrix.sh         | 存储阈值（r² < 0.01 不入库）           |
| THREADS       | 8       | ld_matrix.sh         | PLINK2 线程数                          |

## 下游消费方

| 消费方               | 查询                                           | 用途                      |
|----------------------|------------------------------------------------|---------------------------|
| MiXeR extract        | `WHERE unphased_r2 > 0.8`                     | tag 选择（LD 剪枝）       |
| MiXeR LD fold        | `WHERE unphased_r2 >= 0.05`                   | 充分统计量（m1/m2）       |
| LDSC                 | 全量扫描                                       | LD score 计算             |
| precompute_tags      | `WHERE unphased_r2 > 0.8` + `>= 0.05`         | 预算 tag 面板 + 子图      |

## 验证

```bash
# 检查表是否存在 + 行数
cargo run -p data-engine --bin lake_cli -- count ld_matrix.eur_chr22

# 看表结构
cargo run -p data-engine --bin lake_cli -- schema ld_matrix.eur_chr22

# 抽样
cargo run -p data-engine --bin lake_cli -- query \
    "SELECT * FROM iceberg.ld_matrix.eur_chr22 WHERE unphased_r2 > 0.8 LIMIT 10"
```

## 规模参考

| 染色体 | SNP 数（AF 面板） | LD 对数（r²≥0.01） | Iceberg 表名          |
|--------|-------------------|--------------------|-----------------------|
| chr1   | ~176k             | ~数十 M            | `eur_chr1`            |
| chr22  | ~32k              | ~数 M              | `eur_chr22`           |
| 全基因组| ~10.7M           | ~数亿              | chr1–chr22 共 22 张表  |
