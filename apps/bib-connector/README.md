# Autonomics 浏览器文献导入插件

这是可直接加载的 Chromium Manifest V3 插件，支持 Chrome / Edge 的开发者模式。无需构建或安装 npm 依赖。

## 安装与使用

1. 启动含 `POST /api/v1/bib/articles/manual` 接口的 Autonomics 网关。
2. 打开 `chrome://extensions`（Edge 为 `edge://extensions`），启用开发者模式，选择“加载已解压的扩展程序”，选择本目录（或打包产物 `dist/unpacked`）。
3. 打开论文页面，点击插件按钮。插件会读取当前页的元数据并展示预览。
4. 输入网关地址，例如 `http://127.0.0.1:8765`，以及网关 token（通常位于运行网关机器的 `~/.autonomics/gateway.token`）。点击 **Connect**，授予该网关的访问权限。
5. 核对标题、作者、标识符、年份及期刊；可选择目标集合。点击 **Save metadata** 入库。
6. 网关不可用时可 **Keep offline**。恢复连接后，对待处理条目点击 **Retry**。队列绑定原网关，避免误导入其他库。

如果返回重复记录冲突，插件会保留待处理条目并提示检查库中的记录，不会覆盖已有元数据。确认已入库后，可从待处理列表移除。成功创建后的集合归属失败会保留已创建记录 ID，重试只补集合归属。

## 采集范围

- Highwire `citation_*`、Dublin Core、PRISM、Open Graph 元数据。
- JSON-LD 的 ScholarlyArticle / Article / NewsArticle / Book 标题、作者、年份及摘要。
- PubMed 页面 PMID、arXiv 摘要/PDF URL 中的标识符。
- 普通网页回退到标题和描述；来源 URL（去除 fragment）作为 `other` 标识符保存。
- 作者以发布页面的完整名称保留；需要拆分姓与名时，可在文献元数据编辑器中进一步整理。

这是元数据采集器，不包含站点专用抓取器全集、网页快照保存或受权限保护 PDF 的自动下载。受浏览器保护的页面（如 `chrome://`、扩展商店）不能注入采集脚本。默认不支持直接读取本地文件。

## 凭据与存储

仅在用户点击插件时通过 `activeTab` 读取当前页。未配置全站常驻内容脚本。网关请求由插件页面发出，token 不会注入网页；只在 `chrome.storage.session` 中保留至浏览器会话结束，可随时点击 **Forget token** 清除。待处理元数据保存在本机扩展存储中，卸载插件会清除，不是文献库同步或备份。

HTTP 仅允许 localhost / 127.0.0.1；其他地址要求 HTTPS。请求不携带站点 cookie、不跟随重定向。插件仅按用户指定的网关申请可选主机权限。该 token 沿用现有网关授权范围，并不是单独签发的只读或仅导入 token。

## 打包

```bash
cd apps/bib-connector
npm run build
```

打包只需要 Python 3，不下载依赖。生成可加载目录 `dist/unpacked`、可分发压缩包 `dist/autonomics-bib-connector.zip` 和 `dist/SHA256SUMS`。ZIP 固定文件顺序、时间戳和权限，相同源码可重复生成相同校验和。压缩包只包含运行所需的六个文件，不含测试或开发依赖。

## 测试

在仓库已安装 GUI 测试依赖后运行：

```bash
node --test apps/bib-connector/tests/*.test.mjs
```

浏览器测试通过 `apps/gui` 的 Playwright 启动 Chromium；可用 `PDF_TEST_CHROMIUM` 指定可执行文件。测试覆盖真实页面 DOM 提取、预览编辑、认证参数、目标集合、失败后队列保留、重试与网关隔离。真实 Chrome API 的权限弹窗与用户点击扩展按钮仍应在目标浏览器人工验收；没有发布到扩展商店。

实现参考：[Chrome activeTab](https://developer.chrome.com/docs/extensions/develop/concepts/activeTab)、[扩展跨源请求](https://developer.chrome.com/docs/extensions/develop/concepts/network-requests)、[session 存储](https://developer.chrome.com/docs/extensions/reference/api/storage)。
