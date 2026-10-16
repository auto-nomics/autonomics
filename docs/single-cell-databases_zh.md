# 单细胞数据库接入清单

本文是单细胞外部数据源的统一接入路线图。目标是为每个数据库补齐四层能力：调研文档、Rust SDK、非下载查询 DAG 节点、可选的数据/文件下载节点。查询节点输出 DataFrame；矩阵、FASTQ、H5AD、AnnData 等大文件统一走 VFS/FileSet 下载路径，不与查询 API 混在同一节点。

当前总控清单共 28 项：GEO、CELLxGENE、HCA、SCEA、SCP、HuBMAP、Tabula Sapiens、TISCH、HTAN、KPMP、HCL、Allen Brain 与 NeMO 已完成查询接入，Enrichr 复用现有能力，其余 14 项按后续批次推进。第一批参考图谱主通道、第二批疾病专题与第三批 Allen Brain/NeMO 已完成。

## 接入状态

| 状态 | 含义 |
|---|---|
| 已接入 | SDK 与查询 DAG 节点已进入仓库，并有测试 |
| 调研中 | 已确认入口和主要数据形态，尚未建立 SDK |
| 待接入 | 已列入路线图，尚未开发 |
| 降级接入 | 官方 API 不稳定或无公开 API，优先使用清单/文件下载 |
| 查询层已接入 | SDK 与非下载查询 DAG 节点已进入仓库并有测试；下载节点仍待实现 |

## P0：优先接入

这组数据库规模大、数据标准化好、对参考映射和图谱分析最有价值。

| ID | 数据库 | 层次 | 状态 | 接入重点 | 预期节点 |
|---|---|---|---|---|---|
| `geo` | NCBI GEO / GEO DataSets | 归档 + 研究/样本/文件元数据 | 已接入 | E-utilities `gds`；GSE/GDS/GSM/GPL accession；PubMed/SRA/BioProject 关联 | `source_geo_einfo/esearch/esummary/elink/egquery/espell` 已注册 |
| `cellxgene` | CZ CELLxGENE Discover / Census | 图谱、H5AD、细胞类型标注 | 查询层已接入 | Discover `collections/index`、`collections/{id}`、`datasets/index`、`datasets/{id}/assets`；Census Python/TileDB 查询待后续扩展 | `source_cellxgene_collections/collection/datasets/assets` 已注册；`download_cellxgene_h5ad` 待实现 |
| `hca` | Human Cell Atlas Data Portal / Azul | 人类细胞图谱项目、样本与文件索引 | 查询层已接入 | Azul `/index/projects`、`/index/projects/{id}`、`/index/files`、`/index/samples`；catalog 可用 `HCA_CATALOG` 指定 | `source_hca_projects/project/files/samples` 已注册；`download_hca_matrix` 待实现 |
| `scea` | EBI Single Cell Expression Atlas | 已处理表达矩阵与实验元数据 | 查询层已接入 | `/json/experiments` 索引、实验页内容 JSON、`marker-genes/clusters`；实验设计端点目前在前端代码中标注 TBC | `source_scea_experiments/experiment/markers` 已注册；`download_scea_matrix` 待实现 |
| `scp` | Broad Single Cell Portal | 研究门户与矩阵/元数据 | 查询层已接入 | 公开 `site/studies`、`search`、`site/studies/{accession}`；受保护 `/studies` 系列后续按认证需求扩展 | `source_scp_studies/search/study/files` 已注册；`download_scp_study` 待实现 |
| `hubmap` | HuBMAP | 人体图谱、空间与单细胞数据 | 查询层已接入 | Search API v3 `/search`、Entity API `entities/{id}/globus-url`；公开索引含 dataset 元数据与文件清单 | `source_hubmap_search/dataset/files/globus` 已注册；`download_hubmap_dataset` 待实现 |
| `tabula_sapiens` | Tabula Sapiens | 人体多器官参考图谱 | 查询层已接入 | 官方 figshare article `14267219` 的版本、发布元数据与 31 个资产清单；资产分为 organ H5AD、compartment H5AD、全图谱 H5AD、cell metadata | `source_tabula_sapiens_versions/release/files` 已注册；`download_tabula_sapiens_reference` 待实现 |

## P1：肿瘤与疾病专题

这组数据库对临床问题、肿瘤微环境、疾病细胞类型和大队列比较最有用。

| ID | 数据库 | 层次 | 状态 | 接入重点 | 预期节点 |
|---|---|---|---|---|---|
| `tisch` | TISCH2 | 肿瘤微环境 scRNA-seq/scATAC-seq | 降级接入 | 当前部署无 JSON API：结构化解析官方 Dataset HTML（190 个数据集）与 Documentation 细胞类型表；旧 AJAX 端点已静态化 | `source_tisch_datasets/cell_types/assets` 已注册；dataset 归档与 marker 查询受官方部署限制，待官方接口恢复 |
| `htan` | Human Tumor Atlas Network | 肿瘤单细胞与空间图谱 | 查询层已接入 | Portal 前端使用的公开只读 ClickHouse；默认 `htan_2026_932`，覆盖 14 个 Atlas、221,253 个文件行 | `source_htan_atlases/datasets/files` 已注册；Synapse `download_htan_file` 待实现 |
| `kpmp` | Kidney Precision Medicine Project | 肾脏疾病与参考图谱 | 查询层已接入 | Atlas GraphQL `atlas.kpmp.org/graphql`：数据类型/参与者计数、repository 汇总、participant 临床样本、Explorer/Spatial 数据集、participant 文件清单 | `source_kpmp_data_types/repository/participant/participant_datasets/participant_repository` 已注册；受控文件下载待实现 |
| `hcl` | Human Cell Landscape | 人体多组织单细胞景观 | 查询层已接入 | 官方 figshare article `7235471` 的 4 个版本、9 个资产清单、MD5 与下载 URL；release 行关联 Nature paper、GSE134355、CNP0000325、portal 与代码库 | `source_hcl_versions/release/files` 已注册；`download_hcl_reference` 待实现 |

## P1：器官与神经系统专题

| ID | 数据库 | 层次 | 状态 | 接入重点 | 预期节点 |
|---|---|---|---|---|---|
| `allen_brain` | Allen Brain Cell Types | 脑细胞类型与转录组/电生理 | 查询层已接入 | RMA API `Structure/Gene` 支持脑区分页与基因 symbol 查询；官方 taxonomy index 当前发布 14 个版本化 H5AD 参考，解析 species/assay/region/release date/URL | `source_allen_brain_areas/types/genes` 已注册；`download_allen_brain_matrix/reference` 待实现 |
| `nemo` | NeMO Analytics | 神经系统单细胞与空间组学 | 查询层已接入 | 官方 Dataset Explorer 使用的公开接口：collection 清单、dataset 搜索分页、矩阵形状/H5AD/tarball 元数据；当前公开索引约 2168 个数据集 | `source_nemo_collections/datasets/matrices` 已注册；`download_nemo_matrix` 待实现 |
| `heart_cell_atlas` | Heart Cell Atlas | 心脏细胞图谱 | 待接入 | 版本化 cell type、组织区域、参考矩阵 | `source_heart_cell_atlas_versions/cell_types` |
| `lung_cell_atlas` | Lung Cell Atlas | 肺细胞图谱与整合参考 | 待接入 | atlas 版本、物种、疾病/健康队列、细胞类型 | `source_lung_cell_atlas_versions/references` |
| `gut_cell_atlas` | Gut Cell Atlas | 肠道细胞图谱 | 待接入 | 组织、发育阶段、健康/疾病样本与矩阵 | `source_gut_cell_atlas_datasets/references` |

## P1/P2：模式生物与免疫参考

| ID | 数据库 | 层次 | 状态 | 接入重点 | 预期节点 |
|---|---|---|---|---|---|
| `mca` | Mouse Cell Atlas | 小鼠细胞图谱 | 待接入 | 组织、发育阶段、细胞类型、参考矩阵 | `source_mca_versions/references` |
| `immgen` | ImmGen | 小鼠免疫细胞图谱 | 待接入 | cell type、gene expression、module/reference 标签 | `source_immgen_cell_types/expression/modules` |
| `azimuth` | Azimuth references | 参考映射数据 | 待接入 | 按组织下载参考对象；输出物种、组织、参考版本、modalities | `source_azimuth_references`，`download_azimuth_reference` |

## P2：归档与原始测序

这些数据库常与其他 accession 交叉，作为溯源和原始 FASTQ 层使用。

| ID | 数据库 | 层次 | 状态 | 接入重点 | 预期节点 |
|---|---|---|---|---|---|
| `arrayexpress` | ArrayExpress / BioStudies | 归档元数据与处理文件 | 待接入 | BioStudies API；实验、sample、file 关系 | `source_arrayexpress_experiments/samples/files` |
| `sra` | NCBI Sequence Read Archive | 原始测序 | 待接入 | Run/Experiment/Sample/BioProject 元数据；与 GEO accession 联动 | `source_sra_runs/experiments/samples`，`download_sra_runs` |
| `ena` | ENA | 原始测序与Assembly/reads 元数据 | 待接入 | ENA Portal API；reads、analysis、sample、study | `source_ena_reads/studies/samples`，`download_ena_reads` |

## P2/P3：细胞类型、marker 与本体

这组偏知识层，接入后用于自动注释、差异表达解读和跨库字段标准化。

| ID | 数据库 | 层次 | 状态 | 接入重点 | 预期节点 |
|---|---|---|---|---|---|
| `panglaodb` | PanglaoDB | marker 与数据浏览 | 待接入 | cell type marker、物种、组织、证据；若 API 不稳定则用版本化数据文件 | `source_panglaodb_markers/cell_types` |
| `cellmarker` | CellMarker / CellMarker 2.0 | 细胞类型 marker | 待接入 | species、tissue、cell type、marker、证据来源 | `source_cellmarker_markers/evidence` |
| `msigdb` | MSigDB | 基因集与细胞类型签名 | 待接入 | collection、gene set、版本；现有 MSigDB 数据源保持兼容 | `source_msigdb_collections/genesets` |
| `enrichr` | Enrichr | 基因集库与富集 | 已有 SDK 能力 | 单细胞 marker 富集、细胞类型相关 library | 复用/扩展现有 Enrichr 节点 |
| `cell_ontology` | Cell Ontology | 细胞类型本体 | 待接入 | term、关系、cross-reference、版本 | `source_cell_ontology_terms/ancestors/xrefs` |
| `uberon` | UBERON | 解剖/组织本体 | 待接入 | term、关系、跨库 tissue 标准化 | `source_uberon_terms/ancestors/xrefs` |

## 统一实现要求

1. **SDK 命名**：每个数据库一个 crate，例如 `cellxgene`、`hca`、`hubmap`、`tisch`。
2. **查询节点**：所有非下载数据端点输出 Arrow DataFrame，命名尽量为 `source_<db>_<resource>`。
3. **下载节点**：大文件输出 `FileRef/FileSet`，路径必须进入 VFS 或显式绝对路径；下载器要支持断点、校验、清单和取消。
4. **溯源**：DataFrame 保留数据库、记录 ID、版本、发布/更新时间、license、来源 URL；下载文件保留来源 manifest。
5. **本体标准化**：细胞类型优先关联 Cell Ontology，组织优先关联 UBERON，物种优先关联 NCBI Taxonomy。
6. **测试门槛**：每个 SDK 至少有请求模型、反序列化和错误路径单测；查询节点有注册测试与离线 schema 测试；网络测试默认 ignored。
7. **文档门槛**：每个数据库记录官方 API 入口、认证要求、速率限制、分页方式、日期/版本语义、license、下载格式和大文件限制。
8. **验收样例**：每家至少固定一个稳定 accession/dataset，验证搜索、详情、文件定位和下载元数据；能提供真实矩阵的数据库再补一次文件下载冒烟。

## 建议实施批次

1. **第一批：参考图谱主通道**：`cellxgene`、`hca`、`scea`、`scp`、`hubmap`、`tabula_sapiens`。
2. **第二批：疾病专题与肿瘤微环境**：`tisch`、`htan`、`kpmp`、`hcl`。
3. **第三批：器官/脑/模式生物**：`allen_brain`、`nemo`、三大器官 atlas、`mca`、`immgen`、`azimuth`。
4. **第四批：原始数据溯源**：`arrayexpress`、`sra`、`ena`。
5. **第五批：知识标准化**：`panglaodb`、`cellmarker`、`msigdb`、`cell_ontology`、`uberon`，并与现有 Enrichr 能力对齐。

## 风险与预案

- **无公开 API**：降级为版本化清单/文件下载，仍保留稳定 accession 与校验。
- **接口不稳定或 Cloudflare 阻断**：官方文件/manifest 优先，web scraping 不作为正式接口。
- **许可限制**：接入前记录 license；仅商业友好的数据才支持自动下载和缓存。
- **矩阵格式多样**：统一识别 H5AD、AnnData、MatrixMarket、HDF5、Seurat 对象；解析进容器节点，不在查询节点内展开大矩阵。
- **字段语义漂移**：为每个来源维护 schema adapter，并保留原始 JSON 字段用于溯源。
