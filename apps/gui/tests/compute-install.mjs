import { chromium } from "playwright";
import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import { createServer } from "node:http";
import { existsSync } from "node:fs";
let state = { version: 1, revision: 1, policy: { revision: 1, target_id: "local", target_revision: 1 }, targets: [], data_sources: [] };
const streams = [], requests = [];
const mock = createServer((req, res) => {
  if (req.url === "/api/v1/compute/enroll/stream") {
    let body = "";
    req.on("data", chunk => body += chunk);
    req.on("end", () => {
      requests.push(JSON.parse(body));
      res.writeHead(200, { "Content-Type": "text/event-stream", "Cache-Control": "no-cache" });
      res.flushHeaders(); res.write(": keepalive\r\n\r\n"); streams.push(res);
    });
  } else {
    res.writeHead(200, { "Content-Type": "application/json" });
    res.end(JSON.stringify({ state, deployment: { source_compatible: true }, mounts: [], runs: [] }));
  }
});
await new Promise(resolve => mock.listen(0, "127.0.0.1", resolve));
const origin = "http://127.0.0.1:1457";
const vite = spawn(process.execPath, ["node_modules/vite/bin/vite.js", "--host", "127.0.0.1", "--port", "1457", "--strictPort"], {
  env: { ...process.env, AUTONOMICS_HTTP_API_ADDR: `127.0.0.1:${mock.address().port}` }, stdio: ["ignore", "pipe", "pipe"],
});
let output = "", browser;
vite.stdout.on("data", chunk => output += chunk); vite.stderr.on("data", chunk => output += chunk);
const until = async predicate => {
  for (let i = 0; !predicate(); i++) {
    if (i > 400) throw new Error("Timed out: " + output);
    await new Promise(resolve => setTimeout(resolve, 25));
  }
};
const send = (index, event) => streams[index].write(`data: ${JSON.stringify(event)}\r\n\r\n`);
try {
  await until(() => output.includes(origin));
  browser = await chromium.launch({ executablePath: process.env.PDF_TEST_CHROMIUM || (existsSync("/usr/bin/chromium") ? "/usr/bin/chromium" : undefined), args: ["--no-sandbox", "--disable-features=LocalNetworkAccessChecks"] });
  const page = await browser.newPage({ viewport: { width: 1100, height: 1000 } });
  const errors = []; page.on("pageerror", error => { errors.push(error.message); console.error(error.stack); }); page.setDefaultTimeout(10000);
  await page.route(origin + "/", route => route.fulfill({ contentType: "text/html", body: `<!doctype html><html lang="zh"><head><meta charset="utf-8"><link rel="stylesheet" href="/src/app.css"></head><body><div id="root"></div><script type="module">
    import RefreshRuntime from "/@react-refresh";
    RefreshRuntime.injectIntoGlobalHook(window);
    window.$RefreshReg$ = () => {}; window.$RefreshSig$ = () => (type) => type;
    window.__vite_plugin_react_preamble_installed__ = true;
    const {default: {createElement}} = await import("/node_modules/.vite/deps/react.js");
    const {default: {createRoot}} = await import("/node_modules/.vite/deps/react-dom_client.js");
    const {ComputeEditorView} = await import("/src/features/hpc/compute-editor.tsx");
    const root = createRoot(document.getElementById("root"));
    const mount = () => root.render(createElement(ComputeEditorView));
    window.remountCompute = () => { root.render(null); setTimeout(mount, 50); }; mount();
  </script></body></html>` }));
  page.on("console", message => { if (message.type() === "error") console.error(message.text()); });
  await page.goto(origin);
  async function fillAndSubmit() {
    await page.getByRole("button", { name: "注册 Slurm 集群", exact: true }).click();
    for (const [label, value] of [["标识", "Csu"], ["登录节点 SSH 别名", "Csu"], ["远端 worker 路径", "/home/test/bin/worker"], ["安装包路径（可选）", "/opt/autonomics"], ["远端 Python", "/opt/python3.12"]])
      await page.getByLabel(label, { exact: true }).fill(value);
    await page.getByRole("button", { name: "安装并注册", exact: true }).click();
  }
  await fillAndSubmit(); await until(() => streams.length === 1);
  const panel = page.getByRole("region", { name: "计算目标安装进度" });
  send(0, { type: "progress", stage: "authenticate", message: "正在认证" });
  await panel.getByRole("status").getByText("SSH 身份认证", { exact: true }).waitFor();
  assert.equal(requests[0].deploy_worker, "/opt/autonomics"); assert.equal(requests[0].python, "/opt/python3.12");
  send(0, { type: "progress", stage: "upload", message: "上传中", uploaded_bytes: 52428800, total_bytes: 104857600 });
  await panel.getByRole("progressbar").waitFor();
  await page.waitForFunction(() => document.querySelector('[role="progressbar"]')?.getAttribute("aria-valuenow") === "50");
  assert.equal(await page.getByRole("button", { name: "安装进行中…" }).isDisabled(), true);
  assert.equal(await panel.getByText("安装完成", { exact: true }).count(), 0);
  await page.evaluate(() => window.remountCompute());
  await panel.getByRole("progressbar").waitFor();
  assert.equal(await panel.getByRole("progressbar").getAttribute("aria-valuenow"), "50");
  const frame = Buffer.from(`data: ${JSON.stringify({ type: "progress", stage: "upload", message: "节点消息：文件已接收" })}\r\n\r\n`);
  const split = frame.indexOf(Buffer.from("节点")) + 1;
  streams[0].write(frame.subarray(0, split)); await new Promise(resolve => setTimeout(resolve, 20)); streams[0].write(frame.subarray(split));
  await panel.locator("summary").click(); await panel.getByRole("log").getByText(/节点消息：文件已接收/).waitFor();
  send(0, { type: "error", message: "upload refused: disk full" }); streams[0].end();
  await panel.getByText("安装失败", { exact: true }).waitFor();
  await panel.getByText("upload refused: disk full", { exact: true }).waitFor();
  assert.equal(requests.length, 1, "no automatic deployment retry");
  await fillAndSubmit(); await until(() => streams.length === 2);
  send(1, { type: "progress", stage: "upload", message: "上传完成", uploaded_bytes: 104857600, total_bytes: 104857600 });
  send(1, { type: "progress", stage: "save_target", message: "保存目标" });
  await panel.getByRole("status").getByText("保存计算目标", { exact: true }).waitFor();
  assert.equal(await panel.getByText("安装完成", { exact: true }).count(), 0);
  await page.screenshot({ path: "/tmp/autonomics-compute-install.png", fullPage: true });
  state = { ...state, revision: 2, policy: { revision: 2, target_id: "Csu", target_revision: 1 },
    targets: [{ id: "Csu", revision: 1, name: "Csu", enabled: true, backend: { type: "slurm", settings: { ssh_host: "Csu" } }, data_domains: [] }] };
  send(1, { type: "result", state }); streams[1].end();
  await panel.getByText("安装完成", { exact: true }).waitFor();
  await page.getByText("Csu r1", { exact: true }).waitFor();
  assert.equal(await panel.getByRole("progressbar").getAttribute("aria-valuenow"), "100");
  assert.equal(await panel.getByText("upload refused: disk full", { exact: true }).count(), 0);
  await page.setViewportSize({ width: 390, height: 844 });
  assert.equal(await page.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth), true, "mobile layout must not overflow");
  await page.screenshot({ path: "/tmp/autonomics-compute-install-mobile.png", fullPage: true });
  await fillAndSubmit(); await until(() => streams.length === 3);
  send(2, { type: "progress", stage: "authenticate", message: "正在认证" });
  streams[2].end();
  await panel.getByText(/安装进度连接已断开/).waitFor();
  await panel.getByText("状态待确认", { exact: true }).waitFor();
  assert.equal(requests.length, 3, "dropped streams must not replay deployments");
  assert.deepEqual(errors, []);
  console.log("Compute installation: streaming, percentage, navigation, UTF-8/CRLF, failure and retry passed.");
} finally {
  await browser?.close(); vite.kill(); streams.forEach(stream => stream.end()); mock.closeAllConnections(); await new Promise(resolve => mock.close(resolve));
}
