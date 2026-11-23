import { chromium } from "playwright";
import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import { createServer } from "node:http";
import { existsSync } from "node:fs";
let state = { version: 2, policy: { target_id: "local" }, targets: [], data_sources: [] };
const streams = [], requests = [], targetWrites = [], policyWrites = [], mountWrites = [], deleteWrites = [];
const mock = createServer((req, res) => {
  if (req.url === "/api/v1/compute/enroll/stream") {
    let body = "";
    req.on("data", chunk => body += chunk);
    req.on("end", () => {
      requests.push(JSON.parse(body));
      res.writeHead(200, { "Content-Type": "text/event-stream", "Cache-Control": "no-cache" });
      res.flushHeaders(); res.write(": keepalive\r\n\r\n"); streams.push(res);
    });
  } else if (req.method === "DELETE" && req.url.startsWith("/api/v1/compute/targets/")) {
    deleteWrites.push(req.url);
    if (deleteWrites.length === 1) {
      res.writeHead(409, { "Content-Type": "application/json" });
      res.end(JSON.stringify({ error: "Target deletion failed" }));
      return;
    }
    const id = decodeURIComponent(req.url.split("/").at(-1));
    state = { ...state, deleted_target_ids: [...(state.deleted_target_ids ?? []), id] };
    res.writeHead(200, { "Content-Type": "application/json" });
    res.end(JSON.stringify(state));
  } else if (req.method === "PUT" && req.url === "/api/v1/compute/target-data-source") {
    let body = "";
    req.on("data", chunk => body += chunk);
    req.on("end", () => {
      const request = JSON.parse(body);
      mountWrites.push(request);
      if (mountWrites.length === 1) {
        res.writeHead(409, { "Content-Type": "application/json" });
        res.end(JSON.stringify({ error: "Mount unavailable" }));
        return;
      }
      const target = state.targets.find(target => target.id === request.target.id);
      state = { ...state,
        targets: state.targets.map(entry => entry === target ? {
          ...target, ...request.target, retained_data_sources: [target.data_source], data_source: {
            ...target.data_source, id: "mount-replacement",
            backend: { type: "ssh", alias: request.alias, root: request.root, python: request.python },
          },
        } : entry),
      };
      res.writeHead(200, { "Content-Type": "application/json" });
      res.end(JSON.stringify(state));
    });
  } else if (req.method === "PUT" && ["/api/v1/compute/targets", "/api/v1/compute/policy"].includes(req.url)) {
    let body = "";
    req.on("data", chunk => body += chunk);
    req.on("end", () => {
      const request = JSON.parse(body);
      if (req.url.endsWith("/targets")) {
        targetWrites.push(request);
        state = { ...state, targets: state.targets.map(target =>
          target.id === request.id ? { ...target, ...request } : target
        ) };
      } else {
        policyWrites.push(request);
        state = { ...state, policy: { target_id: request.target_id } };
      }
      res.writeHead(200, { "Content-Type": "application/json" });
      res.end(JSON.stringify(state));
    });
  } else {
    res.writeHead(200, { "Content-Type": "application/json" });
    res.end(JSON.stringify({ state, deployment: { source_compatible: true }, mounts: [] }));
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
  const dialog = page.getByRole("dialog", { name: "接入计算目标" });
  const enrollButton = page.getByRole("button", { name: "接入计算目标", exact: true });
  await enrollButton.click();
  await dialog.waitFor();
  await page.keyboard.press("Escape");
  await dialog.waitFor({ state: "hidden" });
  assert.equal(await enrollButton.evaluate(button => button === document.activeElement), true);
  const hash = "a".repeat(64);
  const workerProfile = { worker: { executable: "/remote/autonomics-worker", executable_sha256: hash,
    expected_source_revision: "revision", plugins_root: "/remote/plugins", plugins_sha256: hash,
    podman: { executable: "/usr/bin/podman", executable_sha256: hash, image_catalog: "/remote/images.json", image_catalog_sha256: hash } } };
  async function fillAndSubmit() {
    await enrollButton.click();
    await dialog.waitFor();
    await page.getByLabel("执行方式", { exact: true }).selectOption("slurm");
    const mountConfig = dialog.getByRole("group", { name: "数据源挂载" });
    await mountConfig.getByLabel("存储根目录绝对路径", { exact: true }).fill("/srv/shared");
    await mountConfig.getByLabel("数据节点 SSH 别名", { exact: true }).fill("cluster-storage");
    await page.getByText("高级选项", { exact: true }).click();
    for (const [label, value] of [["标识", "Csu"], ["登录节点 SSH 别名", "Csu"], ["远端 worker 路径（可选）", "/home/test/bin/worker"], ["安装包路径（可选）", "/opt/autonomics"], ["远端 Python", "/opt/python3.12"]])
      await page.getByLabel(label, { exact: true }).fill(value);
    await dialog.getByLabel("SIF 镜像清单", { exact: true }).fill("/gateway/images.json");
    await mountConfig.getByText("csu://", { exact: true }).waitFor();
    await page.getByRole("button", { name: "安装并注册", exact: true }).click();
  }
  await fillAndSubmit(); await until(() => streams.length === 1);
  const panel = page.getByRole("region", { name: "计算目标安装进度" });
  send(0, { type: "progress", stage: "authenticate", message: "正在认证" });
  await panel.getByRole("status").getByText("SSH 身份认证", { exact: true }).waitFor();
  assert.equal("expected_revision" in requests[0], false);
  assert.deepEqual(requests[0].container_runtime, { type: "sif", image_manifest: "/gateway/images.json", runtime: "apptainer" });
  assert.equal(requests[0].storage_root, "/srv/shared");
  assert.equal(requests[0].data_alias, "cluster-storage");
  assert.equal(requests[0].deploy_worker, "/opt/autonomics"); assert.equal(requests[0].python, "/opt/python3.12");
  send(0, { type: "progress", stage: "upload", message: "上传中", uploaded_bytes: 52428800, total_bytes: 104857600 });
  await panel.getByRole("progressbar").waitFor();
  await page.waitForFunction(() => document.querySelector('[role="progressbar"]')?.getAttribute("aria-valuenow") === "50");
  await dialog.waitFor({ state: "hidden" });
  assert.equal(await enrollButton.isDisabled(), true);
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
  state = { ...state, policy: { target_id: "Csu" },
    targets: [{ id: "Csu", name: "Csu", enabled: true, backend: { type: "slurm", settings: {
      ssh_host: "Csu", storage_root_v1: "/srv/shared", scratch_remote_dir: "/srv/shared/dag-tasks",
      panel_cache_remote_dir: "/srv/shared/panels", artifact_root: "/gateway/artifacts",
    } }, data_domains: [{ id: "csu", ssh_aliases: ["Csu"], remote_root: "/srv/shared/data",
      vfs_prefix: "/data/Csu", retained_remote_root: "/srv/shared/dag-tasks", retained_vfs_prefix: "/data/tasks-csu" }],
    data_source: { id: "Csu", domain: "csu", name: "Csu shared filesystem", read_only: false,
      backend: { type: "ssh", alias: "Csu", root: "/srv/shared/data", python: "python3" } } }] };
  send(1, { type: "result", state }); streams[1].end();
  await panel.getByText("安装完成", { exact: true }).waitFor();
  await page.getByText("Csu", { exact: true }).first().waitFor();
  assert.equal(await panel.getByRole("progressbar").getAttribute("aria-valuenow"), "100");
  assert.equal(await panel.getByText("upload refused: disk full", { exact: true }).count(), 0);
  await page.setViewportSize({ width: 390, height: 844 });
  assert.equal(await page.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth), true, "mobile layout must not overflow");
  await page.screenshot({ path: "/tmp/autonomics-compute-install-mobile.png", fullPage: true });
  await enrollButton.click();
  await dialog.waitFor();
  await dialog.getByText("高级选项", { exact: true }).click();
  const mobileBounds = await dialog.boundingBox();
  assert.ok(mobileBounds.y >= 0 && mobileBounds.y + mobileBounds.height <= 844);
  await dialog.getByRole("button", { name: "取消", exact: true }).click();
  await dialog.waitFor({ state: "hidden" });
  await fillAndSubmit(); await until(() => streams.length === 3);
  send(2, { type: "progress", stage: "authenticate", message: "正在认证" });
  streams[2].end();
  await panel.getByText(/安装进度连接已断开/).waitFor();
  await panel.getByText("状态待确认", { exact: true }).waitFor();
  assert.equal(requests.length, 3, "dropped streams must not replay deployments");
  await page.setViewportSize({ width: 1100, height: 1000 });
  // SSH uses the same automatic enrollment, with no worker paths or scheduler fields.
  await enrollButton.click();
  await dialog.waitFor();
  await dialog.getByText("高级选项", { exact: true }).click();
  await page.getByLabel("执行方式", { exact: true }).selectOption("ssh");
  await page.getByLabel("标识", { exact: true }).fill("ssh-test");
  await dialog.getByLabel("存储根目录绝对路径", { exact: true }).fill("");
  await dialog.getByLabel("数据节点 SSH 别名", { exact: true }).fill("");
  await page.getByLabel("登录节点 SSH 别名", { exact: true }).fill("worker");
  await page.getByLabel("远端 worker 路径（可选）", { exact: true }).fill("");
  await page.getByLabel("安装包路径（可选）", { exact: true }).fill("");
  assert.equal(await dialog.getByRole("button", { name: "安装并注册" }).isDisabled(), true);
  for (const [label, value] of Object.entries({
    "Podman SHA-256": hash, "远端插件目录": "/remote/plugins", "插件目录 SHA-256": hash,
    "离线镜像清单": "/remote/images.json", "镜像清单 SHA-256": hash,
  })) await dialog.getByLabel(label, { exact: true }).fill(value);
  assert.equal(await page.getByLabel("分区（可选）", { exact: true }).count(), 0);
  await page.getByRole("button", { name: "安装并注册", exact: true }).click();
  await until(() => streams.length === 4);
  assert.equal(requests[3].backend, "ssh");
  assert.deepEqual(requests[3].container_runtime.podman, workerProfile.worker.podman);
  assert.equal(requests[3].worker_executable, undefined);
  assert.equal(requests[3].deploy_worker, undefined);
  assert.equal(requests[3].partition, undefined);
  assert.equal(requests[3].storage_root, undefined);
  assert.equal(requests[3].data_alias, undefined);
  state = { ...state, policy: { target_id: "ssh-test" }, targets: [...state.targets,
    { id: "ssh-test", name: "SSH test", enabled: true, backend: { type: "ssh", settings: { ssh_host: "worker", ssh_connect_timeout_secs: 43, typed_worker: workerProfile } },
      data_source: { id: "ssh-test", name: "SSH shared data", read_only: true, backend: { type: "ssh", alias: "worker", root: "/home/user/autonomics-data/ssh-test", python: "python3" } },
      data_domains: [] }] };
  send(3, { type: "result", state }); streams[3].end();
  await panel.getByText("安装完成", { exact: true }).waitFor();
  const targetRow = page.locator("li").filter({ has: page.getByText("ssh-test", { exact: true }) });
  await page.getByText("当前执行目标:ssh-test", { exact: true }).waitFor();
  const editButton = targetRow.getByRole("button", { name: "编辑", exact: true });
  const targetForm = page.getByRole("dialog", { name: "编辑目标 ssh-test" });
  await editButton.click();
  const targetMount = targetForm.getByRole("region", { name: "数据源挂载" });
  await targetMount.getByText("/data/ssh-test", { exact: true }).waitFor();
  await targetMount.getByText("/home/user/autonomics-data/ssh-test", { exact: true }).waitFor();
  await targetMount.getByText("只读", { exact: true }).waitFor();
  await targetForm.getByLabel("名称", { exact: true }).fill("Discarded edit");
  await targetForm.getByRole("button", { name: "取消", exact: true }).click();
  await targetForm.waitFor({ state: "hidden" });
  assert.equal(await editButton.evaluate(button => button === document.activeElement), true);
  assert.equal(targetWrites.length, 0);
  await editButton.click();
  assert.equal(await targetForm.getByLabel("名称", { exact: true }).inputValue(), "SSH test");
  await targetForm.getByLabel("名称", { exact: true }).fill("   ");
  assert.equal(await targetForm.getByRole("button", { name: "保存配置", exact: true }).isDisabled(), true);
  await targetForm.getByLabel("名称", { exact: true }).fill(" Updated SSH target ");
  await targetForm.getByRole("button", { name: "保存配置", exact: true }).click();
  await targetForm.waitFor({ state: "hidden" });
  await targetRow.getByText("Updated SSH target", { exact: true }).waitFor();
  assert.equal(targetWrites.length, 1);
  assert.equal(state.targets.filter(target => target.id === "ssh-test").length, 1);
  assert.equal(targetWrites[0].backend.type, "ssh");
  assert.equal(targetWrites[0].name, "Updated SSH target");
  assert.equal(targetWrites[0].backend.settings.ssh_connect_timeout_secs, 43);
  assert.equal(targetWrites.every(request => !("expected_revision" in request)), true);
  assert.deepEqual(policyWrites, [], "editing the selected target must not require reselection");
  assert.equal(await page.getByRole("button", { name: "挂载 SSH 数据源", exact: true }).count(), 0);
  await editButton.click();
  assert.equal(await targetForm.getByLabel("执行模式").count(), 0);
  assert.equal(await targetForm.getByLabel("Podman 可执行文件").inputValue(), "/usr/bin/podman");
  assert.deepEqual(targetWrites.at(-1).backend.settings.typed_worker, workerProfile);
  await targetMount.getByRole("button", { name: "编辑挂载", exact: true }).click();
  const remoteRoot = targetMount.getByLabel("远端根目录", { exact: true });
  assert.equal(await remoteRoot.inputValue(), "/home/user/autonomics-data/ssh-test");
  await remoteRoot.fill("/srv/new-data");
  const save = targetForm.getByRole("button", { name: "保存配置", exact: true });
  await save.click();
  await targetForm.getByRole("alert").getByText("Mount unavailable").waitFor();
  assert.equal(await remoteRoot.inputValue(), "/srv/new-data");
  await save.click();
  await targetForm.waitFor({ state: "hidden" });
  assert.equal(mountWrites[1].target.id, "ssh-test");
  assert.equal(mountWrites[1].root, "/srv/new-data");
  await editButton.click();
  await targetMount.getByText("/data/mount-replacement", { exact: true }).waitFor();
  await targetMount.getByText("/data/ssh-test", { exact: true }).waitFor();
  await page.keyboard.press("Escape");
  await targetForm.waitFor({ state: "hidden" });
  const deleteButton = targetRow.getByRole("button", { name: "删除", exact: true });
  assert.equal(await deleteButton.isDisabled(), true);
  const slurmRow = page.getByRole("listitem").filter({ has: page.getByText("Csu", { exact: true }) });
  await slurmRow.getByRole("button", { name: "选择", exact: true }).click();
  await page.getByText("当前执行目标:Csu", { exact: true }).waitFor();
  await slurmRow.getByRole("button", { name: "编辑", exact: true }).click();
  const unifiedTarget = page.getByRole("dialog", { name: "编辑目标 Csu" });
  const storagePaths = unifiedTarget.getByRole("region", { name: "存储路径" });
  for (const path of ["/srv/shared", "/srv/shared/dag-tasks", "/data/tasks-csu", "/srv/shared/work",
    "/srv/shared/images/sif", "/srv/shared/runtime/apptainer", "/srv/shared/panels", "/gateway/artifacts", "状态目录/compute/targets/Csu/tasks"]) {
    await storagePaths.getByText(path, { exact: true }).waitFor();
  }
  const unifiedMount = unifiedTarget.getByRole("region", { name: "数据源挂载" });
  for (const path of ["/srv/shared/data", "/data/Csu", "csu://"]) {
    await unifiedMount.getByText(path, { exact: true }).waitFor();
  }
  const sifRuntime = unifiedTarget.getByLabel("容器运行时");
  assert.equal(await sifRuntime.inputValue(), "apptainer");
  await sifRuntime.selectOption("singularity_ce");
  await unifiedTarget.getByLabel("SIF 镜像清单").fill("/gateway/sif.json");
  await unifiedTarget.getByLabel("SingularityCE 可执行文件").fill("/opt/singularity/bin/singularity");
  await unifiedTarget.getByRole("button", { name: "保存配置", exact: true }).click();
  await unifiedTarget.waitFor({ state: "hidden" });
  assert.equal(targetWrites.at(-1).backend.settings.sif_runtime, "singularity_ce");
  assert.equal(targetWrites.at(-1).backend.settings.singularity_executable, "/opt/singularity/bin/singularity");
  assert.equal(targetWrites.at(-1).backend.settings.apptainer_manifest, "/gateway/sif.json");
  const deleteDialog = page.getByRole("alertdialog", { name: "删除计算目标 Updated SSH target？" });
  await deleteButton.click();
  await deleteDialog.getByRole("button", { name: "取消", exact: true }).click();
  await deleteDialog.waitFor({ state: "hidden" });
  assert.deepEqual(deleteWrites, []);
  await deleteButton.click();
  const confirmDelete = deleteDialog.getByRole("button", { name: "确认删除", exact: true });
  await confirmDelete.click();
  await deleteDialog.getByRole("alert").getByText("Target deletion failed").waitFor();
  await confirmDelete.click();
  await deleteDialog.waitFor({ state: "hidden" });
  await targetRow.waitFor({ state: "hidden" });
  assert.deepEqual(deleteWrites, Array(2).fill("/api/v1/compute/targets/ssh-test"));
  const otherSources = page.getByRole("region", { name: "其他已挂载数据源" });
  assert.equal(await otherSources.getByText("/data/ssh-test", { exact: true }).count(), 0);
  assert.equal(await otherSources.getByText("/data/mount-replacement", { exact: true }).count(), 0);
  assert.deepEqual(errors, []);
  console.log("Compute configuration: enrollment streaming, target editing, and attached data-source configuration passed.");
} catch (error) {
  const page = browser?.contexts()[0]?.pages()[0];
  console.error(await page?.locator("body").innerText());
  throw error;
} finally {
  await browser?.close(); vite.kill(); streams.forEach(stream => stream.end()); mock.closeAllConnections(); await new Promise(resolve => mock.close(resolve));
}
