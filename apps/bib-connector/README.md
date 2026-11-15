# Autonomics 浏览器文献导入插件

可直接加载的 Chromium Manifest V3 插件（Chrome / Edge 开发者模式），架构对齐 Zotero Connector：工具栏一键保存、翻译器注册表、页面内进度窗、多条目选择、PDF 附件自动上传、离线队列。无需构建或安装 npm 依赖（打包仅需 Python 3）。

## 使用

1. 启动含 `/api/v1/bib/*` 接口的 Autonomics 网关。
2. `chrome://extensions`（Edge 为 `edge://extensions`）启用开发者模式，"加载已解压的扩展程序"选择本目录（或 `dist/unpacked`）。
3. 首次使用右键工具栏图标 → "Connector settings…"（或未配置时直接点按钮）打开设置页：填网关地址（如 `http://127.0.0.1:8765`）与 token（网关机器的 `~/.autonomics/gateway.token`），点 **Connect**。
4. 勾选 **Detect pages automatically** 后，插件会申请全站读取权限并在每次导航后自动检测页面；工具栏按钮显示检测结果：绿点 `●` 表示可保存条目、数字角标表示多结果列表页、灰 `×` 表示网关不可达。
5. 在论文页点击工具栏按钮或按 **Ctrl+Shift+S**（或右键菜单 "Save to Autonomics"）即保存：页面内弹出进度窗，元数据入库后自动下载 `citation_pdf_url` 指向的 PDF 并作为首选附件上传。
6. 列表页（PubMed 检索、arXiv 列表、COinS 页面）会在进度窗内给出勾选列表，确认后逐条抓取保存。
7. 网关不可达时自动落离线队列；设置页（右键图标 → Connector settings…）可重试。队列绑定原网关，避免误导入其他库。

## 架构（对齐 Zotero Connector）

- **`background.js`** — MV3 service worker，是保存管线的中枢：网关探活（`GET /health`）驱动每 tab 图标状态、tabInfo 序列化到 `storage.session` 以扛 SW 重启、右键菜单按导航重建、元数据建档（`POST /articles/manual`）与 PDF 附件上传（multipart → `/articles/{id}/attachments`）都在 worker 内完成，token 不进页面。
- **`translators.js`** — 翻译器注册表，契约模仿 Zotero 的 `detectWeb`/`doWeb`：PubMed、arXiv、COinS、Embedded Metadata（Highwire/Dublin Core/PRISM/Open Graph/JSON-LD）、Web Page 回退，按 priority 排序，`detect` 返回 `"multiple"` 时进入条目选择流程。因为翻译器是第一方代码，无需 Zotero 的 repo 分发与沙箱 translate host。
- **`content.js`** — 内容脚本（仅在全站权限授予后注入）：在页面 DOM 上跑检测与翻译（MV3 service worker 没有 `DOMParser`，这是 Zotero 需要 offscreen host 的原因；我们把 DOM 工作留在页面内解决），并在 closed shadow root 中挂载进度窗 iframe，页面脚本无法触及。
- **`progress.html/progress.js`** — 页内进度窗（web-accessible iframe + nonce 鉴权）：保存进度逐条显示，多结果时内嵌 Zotero itemSelector 式勾选列表。
- **`popup.html/popup.js`** — 设置页兼离线队列管理；未配置网关时兼作按钮弹窗。

不包含：站点专用抓取器全集、网页快照（SingleFile）、RIS/BibTeX 文件拦截导入、Google Docs 集成、扩展商店发布。受浏览器保护页面不能注入。授权全站权限后，已打开的页面需刷新才会注入内容脚本。

## 凭据与存储

网关请求全部由 service worker / 扩展页发出；token 只存 `chrome.storage.session`（浏览器会话结束即失效），可随时 **Forget token**。HTTP 仅允许 localhost / 127.0.0.1，其他地址要求 HTTPS；请求不带站点 cookie、不跟随重定向。不勾选自动检测时不申请任何全站权限，退回 activeTab 手动模式（无自动图标状态，保存仍可用）。

## 打包与测试

```bash
cd apps/bib-connector
npm run build        # 生成 dist/unpacked、dist/autonomics-bib-connector.zip、dist/SHA256SUMS（可复现）
node --test tests/*.test.mjs
```

测试（headless Chromium，经 apps/gui 的 Playwright，可用 `PDF_TEST_CHROMIUM` 指定可执行文件）覆盖：翻译器真实 DOM 检测（出版社/PubMed/arXiv/COinS/普通页面）、popup 凭据与含 PDF 附件的保存重试、进度窗渲染与选择、以及真实扩展加载的 E2E（检测→图标→保存→附件上传）。真实 Chrome 的权限弹窗与工具栏点击仍需人工验收。

实现参考：[Zotero Connector 源码](https://github.com/zotero/zotero-connectors)、[Chrome activeTab](https://developer.chrome.com/docs/extensions/develop/concepts/activeTab)、[扩展跨源请求](https://developer.chrome.com/docs/extensions/develop/concepts/network-requests)、[session 存储](https://developer.chrome.com/docs/extensions/reference/api/storage)。
