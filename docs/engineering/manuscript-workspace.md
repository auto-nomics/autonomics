# 文献库内的 LaTeX 手稿工作区

手稿的主数据是多文件 LaTeX 源码，不再是 `writing-types::Document` 结构树。工作台的文献区域新增 **LaTeX 手稿** 入口，采用文件树 / 源码编辑 / PDF 预览的布局，引用来自同一文献库。

## 使用流程

1. 在文献区打开「LaTeX 手稿」，创建项目。默认包含 `main.tex` 与 `references.bib`。
2. 编辑源码，使用保存按钮或 Ctrl/Cmd+S 保存。可以增加章节文件并用 `\input{...}` 引用，上传图表作为项目附件。
3. 打开「文献库与引用」，搜索并插入文献。系统按文献 ID 分配稳定引用键，原子更新项目关联与 `references.bib`。
4. 点击「编译」。先保存本地修改，再由内嵌 Tectonic 的隔离 worker 编译保存的项目快照。PDF、日志、诊断和来源版本一并保存。
5. 让智能体读取项目、提出修改或添加评论。智能体的修改进入「审阅」面板，人工查看差异后接受或拒绝。陈旧建议不能覆盖新版本。
6. 使用版本历史查看和恢复源码。恢复生成新版本，不删除后续历史。PDF 和源码 ZIP 可导出。

## 数据与协作边界

- `manuscript-core`：持久化项目、源码、附件、不可变版本、修改建议、评论、编译记录。项目聚合使用 compare-and-swap，避免静默覆盖。二进制附件独立存储，项目原子发布附件引用。
- `BibShared.manuscripts`：由文献管理基础设施拥有并共享；所有手稿数据由共享 VFS 保存，根目录为 `/manuscripts`；项目列表通过扫描项目目录生成，不使用 Turso 索引。
- `api-server::manuscripts`：已有认证边界下的 `/bib/manuscripts` API。HTTP 人工编辑固定记录 `human`，不信任请求中传入的作者字段。
- `runtime::manuscript_tools`：`manuscript_list/read/create/propose/comment/compile`。作者来自真实 agent path。没有让智能体接受自己的建议或直接覆盖人工源文件的工具。
- GUI 本地草稿与服务器版本分开保存，冲突时保留草稿并展示对照。人工确认基线后才能重新保存。会话级草稿恢复保留原始版本号，关闭标签后重新打开仍可恢复编辑；恢复不会绕过冲突检查。
- 评论记录源码版本与行区间；原文变化会标记位置需复核。当前采用版本控制与刷新机制，不声称具备 Overleaf 的实时多人光标、字符级 CRDT、完整 SyncTeX 双向定位或 Word 协作功能。

## VFS 文件协议（v1）

```text
/manuscripts/
  <project-id>/
    project.json                    # protocol=1、generation、snapshot
    snapshots/<snapshot-id>/
      metadata.json                 # 项目、文件版本、附件映射、批注、审阅建议
      history.json                  # 源文件历史与删除记录
      files/
        main.tex
        references.bib
        figures/figure.png          # 完整可编译的源码与附件目录
    builds/<build-id>/
      build.json                    # 状态、诊断、源码版本、创建时间
      compile.log
      output.pdf                    # 仅构建产生 PDF 时存在
    latest-build.json                # 最近完成的构建 ID
  assets/<asset-id>                  # 不可变附件版本
```

`project.json` 是项目提交入口。保存先写入一个新的完整快照，成功后对该清单做 compare-and-swap。源码正文只放在 `.tex` / `.bib` 等文件中，`metadata.json` 中的当前正文为空；历史正文保存在 `history.json`。扫描时忽略尚未发布清单的目录；已发布但损坏的项目会报错，不会当成空项目。

读取项目、导出和编译均固定使用清单指向的同一个快照。快照目录不可原地修改；外部工具应通过手稿 API 提交修改，或者遵循完整快照加清单发布协议。共享 VFS 句柄及其 clones 内的并发写入受到协调；当前不支持多个独立进程同时写入同一项目根目录。存储后端需要支持 VFS 的原子替换（rename）能力。未提交快照和旧快照暂不自动回收。

独立 `ManuscriptStore::open(directory)` 直接将传入目录作为本地 VFS 根目录，项目保存在其 `manuscripts/` 子目录。无共享 VFS 的 `BibShared` 使用文献库数据库同级的 `manuscripts-vfs/` 目录。运行时使用已经挂载的共享 VFS。测试使用临时 VFS。

## 编译与部署

`manuscript-engine` 链接官方 Tectonic Rust 内核，包含 XeTeX、BibTeX、xdvipdfmx。应用重启自身的内部 worker 入口执行编译，正式 CLI 与桌面入口在初始化运行时前分派该模式，不依赖系统安装的 `tectonic` 命令，也不会回退为假成功。

当前隔离执行支持 Linux Landlock。首次使用需要下载固定版本的 TeX 支持资源，之后复用缓存；离线资源与自定义 bundle 的配置及构建依赖见 [编译器说明](../../crates/manuscript-engine/README.md)。没有支持的隔离能力或缺少资源时，界面显示具体编译错误。

编译使用独立临时目录、禁用 shell escape、限制文件访问、时间与资源使用；取消请求会终止 worker。HTTP 与智能体共享编译并发限制。每个 PDF 都绑定构建时的完整源码和附件版本。

## 存储边界

手稿模块没有 Turso 依赖、数据库表、旧库导入器或数据库路径配置。启动不再读取 `.manuscripts.db` 或旧 `writing.db`；已有 VFS 项目继续按文件协议读取。旧数据库文件不会自动删除。文献库本身仍保留自己的数据库，与手稿存储分开。

## 验证入口

- `cargo test -p manuscript-core --lib`
- `cargo test -p api-server --lib manuscripts`
- `cargo test -p runtime --lib manuscript_`
- `AUTONOMICS_MANUSCRIPT_WORKER="$PWD/target/debug/autonomics-manuscript-worker" cargo test -p api-server --lib real_http_compile_persists_pdf_with_source_and_asset_revisions -- --ignored --nocapture`（先构建 worker；真实 HTTP 编译与持久化 PDF）
- `cargo test -p manuscript-engine --test embedded -- --ignored --nocapture`（真实 Tectonic，首次需要网络资源）
- `cd apps/gui && npm run build`
- `cd apps/gui && npm run test:manuscripts`（HTTP fixture 驱动，真实 Monaco 与 PDF.js 渲染）

设计参考：[Overleaf 审阅](https://www.overleaf.com/learn/how-to/Track_Changes_in_Overleaf)、[版本历史](https://www.overleaf.com/learn/latex/Using_the_History_feature)、[Tectonic Rust driver](https://docs.rs/tectonic/0.17.0/tectonic/driver/index.html)。
