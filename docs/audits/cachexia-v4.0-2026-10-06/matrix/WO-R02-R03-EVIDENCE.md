# WO-R-02 / WO-R-03 收口证据（2026-10-06）

对应矩阵行 1（twosamplemr）与行 4（mrlap_official）。本文件只登记证据身份与未结项，不重写矩阵 v1 封口口径。

## WO-R-03 TwoSampleMR seed 冻结 —— 已关闭（部署完成）

- 机制：v0.7.9 全部随机消耗点仅在 `mr()` 内——weighted/simple median 经
  `weighted_median_bootstrap`（nboot=1000）、mode 家族经 `mr_mode.R` 的
  `boot()`、egger bootstrap 经向量化 rnorm 矩阵；`harmonise_data` 不读 RNG。
  因此 RNGkind(Mersenne-Twister/Inversion/Rejection)+`set.seed(seed)` 置于
  mr() 调用前即覆盖全部 bootstrap 量，对确定性方法零影响。
- 代码：`ZJ-2002/twosamplemr-plugin` main=9d4098f（seed 参数 default=1，
  int 1..2147483647；空 seed 保留旧行为）；仓内自含
  `test_seed_determinism.sh`。
- 部署：plugins.toml twosamplemr 已由并行会话改钉个人仓
  rev=074900f（=9d4098f+镜像改钉叠加），部署 checkout 实测含 set.seed ×2、
  脚本经 loader 内联装载；镜像 ghcr.io/zj-2002/twosamplemr@sha256:4ffdb466…
  上功能测试四项全绿（同 seed 双跑 results/harmonised/RDS 逐字节一致；
  seed 1vs2 仅 median/mode SE 动、IVW/Egger 逐位不动；空 seed 双跑发散=
  旧缺口实锤；seed=0 双层拒绝）。
- 参照锚：X01→M06 frozen_primary（39 IV）IVW P=0.814965093902353 与 A 验收
  冻结值一致（旧镜像 c270de99 上实测）。
- 契约锁：`crates/container-plugin/tests/twosamplemr_migration.rs` 钉
  env 默认渲染"1"、提交值 4242 渲染、RNGkind/set.seed 脚本标记；迁移族
  5/5 绿、container-plugin 全量绿。脚本改动=指纹改动=一次 twosamplemr
  缓存失效（预期成本，WO-R09 机制）。

## WO-R-02 mrlap_official 容器插件 —— 验证关闭，部署待三项 owner 动作

- 代码：`ZJ-2002/mrlap-plugin` main=ce56172。kind `mrlap_official`：一实例
  一边；三文件入（exp/out/IV 列表）四出（17 字段 summary/RDS/txt/log）；
  do_pruning=FALSE 合同固定不暴露；canonical v2 移植（per-edge
  RNGkind+set.seed(seed) 于 MRlap() 前、g() 防御构造、NA 不降 0）。
- 镜像钉扎：ghcr.io/zj-2002/mrlap-official@sha256:78f195b862a3b7c5f8d96a96576cfb2c66b08f9bd31a739da59a017726cd063d。
  R 4.6.1；MRlap 0.0.3.3@660f026（=验收 RemoteSha，sha256:42e74c0c…）、
  TwoSampleMR 0.7.11@d4df219（sha256:a203d741…）、GenomicSEM 0.0.5@da95d431
  （sha256:cf5f1a7f…）按 host 库 RemoteSha 逐一对账钉死；支撑栈 CRAN 快照
  2026-09-13；ieugwasr 留 CRAN 1.1.0（镜像内版本低于 TwoSampleMR 声明的
  ≥1.2.0——与 host 库同态，因 repos=NULL 安装不校验声明；这只证明冻结
  离线路径三边复现可用，不得外推为完整依赖兼容声明；构建期 stopifnot
  三版本）。
- 内核裁定记录：首个 OpenBLAS 构建（digest 841f851b…，已推送但弃用——
  不再作钉）容器内双跑 summary+txt 逐字节一致（seed 合同成立），但对照
  host 冻结参照仅 corrected SE/P 两列动 ~1e-9/1e-8、其余 49 字段逐位同
  =纯 BLAS 指纹；host seed1 环境用参考 BLAS/LAPACK 3.12.0 单线程，故重建
  切 alternatives 对齐内核 → 容器 parity 51/51 ≤1e-12（host 模式先行
  37 PASS 对照同构）。
- parity：`test_mrlap_official.sh` host 模式 51/51、容器模式（终态镜像）
  51/51，对 `WOE4_mrlap_summary_seed1.tsv`；脚本与三边数据身份沿用
  seed1 验收（feeds SHA、x39/m475 IV 列表、45 文件面板身份）。
- 面板：211 MB 发布 `ZJ-2002/catalog-mrlap-ldsc-eur-w-ld-hm3-no-mhc`
  v1，digest sha256:c1bdd30eabb9d821fe06ff5647bf6a08f9db29f80223762cfc6b3eb285c6d065；
  本地缓存已 materialize（bundle 目录+manifest.json+完成标记+
  index.json generation 5，`autonomics-catalog list` 可见）。M_5_50
  实测每染色体仅数字节（真值如 6 字节），非截断。

### 未结项（部署门槛，全部 owner 动作或跨会话动作）

1. ghcr 包可见性 private→public：fine-grained token 无 packages 管理权
   （GET 可见、PATCH 404），网页一键；与 org 系 8 新包同类挂账。
2. 包仓根 index.json 是 registry 形式（publish 在中心提交失败前写入），
   `install` 直读校验要求 `repositories:[]`；REST commit 提交被端点静默
   丢弃，需具备写权的通道修正（引擎运行只读本地缓存，不阻本机部署）。
3. 中心 registry `wjixiang/catalog-index` 收录本 bundle：需
   `create_pr=1` PR（他主仓）。
4. 部署：plugins.toml 添加 mrlap 家族 + daemon 重启（manifest 装载），
   随后正式 DAG 三边复现 = 矩阵行 4 翻 ACCEPTED 的最后条件。
5. 技术≠科学：以上全绿仍不自动构成 G0；不得把容器 parity 宣称为原生
   Rust `mrlap` 的一致认证（行 3 仍 FORBIDDEN）。

## 证据文件位置

- 日志：/tmp/mrlap_parity2_20261006.log（host 51/51）、
  /tmp/mrlap_parity_ctr2_20261006.log（容器 51/51）、
  /tmp/mrlap_parity_ctr_20261006.log（OpenBLAS 6 字段 BLAS 指纹记录，留证）、
  /tmp/wor02_ctr_det.log（OpenBLAS 内双跑逐字节）、
  /tmp/wor03/run_test.sh+summary.txt（WO-R-03 首验，X01→M06 冻结参照核对）、
  /tmp/mrlap_build{2,3}.log、/tmp/mrlap_push3.log、/tmp/mrlap_panel_publish*.log。
- 本机镜像：localhost/mrlap-official:0.0.3.3 = 294152508a32 = 钉扎 digest
  78f195b8… 同内容。
- 面板本地缓存：~/.autonomics/panels/ZJ-2002/catalog-mrlap-…@sha256:c1bdd30e…
  （index.json.bak-before-mrlap 留原代次）。
