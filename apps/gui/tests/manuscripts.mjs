import { chromium } from "playwright";
import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import { once } from "node:events";
import { existsSync, mkdirSync } from "node:fs";
const origin = "http://127.0.0.1:1443";
const server = spawn(
  process.execPath,
  [
    "node_modules/vite/bin/vite.js",
    "--host",
    "127.0.0.1",
    "--port",
    "1443",
    "--strictPort",
  ],
  { stdio: ["ignore", "pipe", "pipe"] },
);
let output = "";
server.stdout.on("data", (value) => (output += value));
server.stderr.on("data", (value) => (output += value));
let browser;
const initial =
  "\\documentclass{article}\n\\title{Evidence and discovery}\n\\begin{document}\n\\maketitle\n\\section{Introduction}\nHuman and agent collaboration.\n\\section{Methods}\nDescribe the evidence.\n\\bibliographystyle{plain}\n\\bibliography{references}\n\\end{document}\n";
const file = {
  path: "main.tex",
  content: initial,
  revision: 1,
  updated_at: "2026-11-15",
};
const project = {
  id: "p1",
  title: "Evidence and discovery",
  main_file: "main.tex",
  article_ids: [],
  created_at: "2026-11-15",
  updated_at: "2026-11-15",
};
const proposal = {
  id: "suggestion-1",
  path: "main.tex",
  base_revision: 1,
  original: initial,
  proposed: initial.replace(
    "Describe the evidence.",
    "Describe the evidence and report uncertainty.",
  ),
  status: "pending",
  author: "research-agent",
  summary: "Clarify the methods",
  created_at: "2026-11-15",
};
const comments = [],
  requests = [],
  assets = [];
let conflictNext = false,
  compileRelease;
const makePdf = () => {
  const content =
    "BT /F1 20 Tf 70 750 Td (Evidence and discovery) Tj 0 -35 Td /F1 12 Tf (Compiled PDF) Tj ET";
  const objects = [
    "<< /Type /Catalog /Pages 2 0 R >>",
    "<< /Type /Pages /Kids [3 0 R] /Count 1 >>",
    "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 595 842] /Resources << /Font << /F1 4 0 R >> >> /Contents 5 0 R >>",
    "<< /Type /Font /Subtype /Type1 /BaseFont /Times-Roman >>",
    `<< /Length ${content.length} >>\nstream\n${content}\nendstream`,
  ];
  let pdf = "%PDF-1.4\n";
  const offsets = [0];
  objects.forEach((object, index) => {
    offsets.push(Buffer.byteLength(pdf));
    pdf += `${index + 1} 0 obj\n${object}\nendobj\n`;
  });
  const start = Buffer.byteLength(pdf);
  pdf += `xref\n0 6\n0000000000 65535 f \n${offsets
    .slice(1)
    .map((offset) => `${String(offset).padStart(10, "0")} 00000 n \n`)
    .join("")}trailer\n<< /Size 6 /Root 1 0 R >>\nstartxref\n${start}\n%%EOF`;
  return Buffer.from(pdf);
};
try {
  for (let i = 0; !output.includes(origin); i++) {
    if (i > 200 || server.exitCode !== null) throw new Error(output);
    await new Promise((resolve) => setTimeout(resolve, 50));
  }
  browser = await chromium.launch({
    executablePath:
      process.env.PDF_TEST_CHROMIUM ||
      ["/usr/bin/chromium", "/opt/google/chrome/chrome"].find(existsSync),
    args: ["--no-sandbox"],
  });
  const page = await browser.newPage({
    viewport: { width: 1500, height: 960 },
  });
  const errors = [];
  page.on("pageerror", (error) => errors.push(error.message));
  await page.route(`${origin}/`, (route) =>
    route.fulfill({
      contentType: "text/html",
      body: '<html><body style="margin:0"></body></html>',
    }),
  );
  await page.route("**/api/v1/**", async (route) => {
    const request = route.request(),
      path = new URL(request.url()).pathname;
    const body = request.postData() ? request.postDataJSON() : undefined;
    requests.push({ path, body, method: request.method() });
    if (path === "/api/v1/bib/manuscripts")
      return route.fulfill({
        json: request.method() === "POST" ? project : [project],
      });
    if (path.endsWith("/builds/latest"))
      return route.fulfill({ status: 404, json: { error: "No build" } });
    if (path.endsWith("/export"))
      return route.fulfill({
        json: {
          filename: "manuscript-source.zip",
          data_base64: Buffer.from("PK fixture").toString("base64"),
        },
      });
    if (path.endsWith("/pdf"))
      return route.fulfill({ contentType: "application/pdf", body: makePdf() });
    if (path.endsWith("/compile")) {
      await new Promise((resolve) => {
        compileRelease = resolve;
      });
      return route.fulfill({
        json: {
          id: "build1",
          main_file: "main.tex",
          status: "succeeded",
          log: "Tectonic compilation complete",
          diagnostics: [],
          source_revisions: { "main.tex": file.revision },
        },
      });
    }
    if (path.endsWith("/files")) {
      if (conflictNext) {
        conflictNext = false;
        file.revision++;
        file.content = initial + "% Remote update\n";
        return route.fulfill({
          status: 409,
          json: { error: "Revision conflict" },
        });
      }
      assert.equal(body.expected_revision, file.revision);
      file.content = body.content;
      file.revision++;
      return route.fulfill({ json: file });
    }
    if (path.endsWith("/resolve")) {
      proposal.status = body.action === "accept" ? "accepted" : "rejected";
      if (body.action === "accept") {
        file.content = proposal.proposed;
        file.revision++;
      }
      return route.fulfill({ json: proposal });
    }
    if (path.endsWith("/comments") && request.method() === "POST") {
      comments.push({
        id: "c1",
        ...body,
        revision: file.revision,
        author: "human",
        resolved: false,
        created_at: "2026-11-15",
      });
      return route.fulfill({ json: comments[0] });
    }
    if (path.endsWith("/comments/c1")) {
      comments[0].resolved = body.resolved;
      return route.fulfill({ json: comments[0] });
    }
    if (path.endsWith("/history"))
      return route.fulfill({
        json: [
          {
            id: "r1",
            path: "main.tex",
            content: initial,
            revision: 1,
            actor: "human",
            message: "Initial draft",
            created_at: "2026-11-15",
          },
        ],
      });
    if (path.endsWith("/restore")) {
      assert.equal(body.expected_revision, file.revision);
      file.content = initial;
      file.revision++;
      return route.fulfill({ json: file });
    }
    if (path.endsWith("/articles"))
      return route.fulfill({
        json: {
          articles: [
            {
              id: "article1",
              title: "Collaborative scientific writing",
              authors: [{ last_name: "Smith" }],
              year: 2026,
            },
          ],
        },
      });
    if (path.endsWith("/references")) {
      project.article_ids = body.article_ids;
      return route.fulfill({
        json: {
          file: {
            path: "references.bib",
            content:
              "@article{smith2026,title={Collaborative scientific writing}}",
            revision: 1,
          },
          keys: { article1: "smith2026" },
        },
      });
    }
    if (path.endsWith("/assets")) {
      if (request.method() === "PUT") {
        assets.push({
          path: body.path,
          revision: 1,
          byte_size: 32,
          updated_at: "2026-11-15",
        });
        return route.fulfill({ json: assets[0] });
      }
      assets.length = 0;
      return route.fulfill({ json: {} });
    }
    if (path === "/api/v1/bib/manuscripts/p1")
      return route.fulfill({
        json: {
          project,
          files: [file],
          proposals: [proposal],
          comments,
          assets,
        },
      });
    return route.fulfill({
      status: 404,
      json: { error: `Unexpected ${path}` },
    });
  });
  await page.goto(origin);
  await page.evaluate(async () => {
    await import("/src/app.css");
    (await import("/src/lib/theme.ts")).setThemeMode("light");
    const { default: refresh } = await import("/@react-refresh");
    refresh.injectIntoGlobalHook(window);
    window.$RefreshReg$ = () => {};
    window.$RefreshSig$ = () => (type) => type;
    window.__vite_plugin_react_preamble_installed__ = true;
    const { mountManuscripts } = await import("/tests/manuscripts-mount.tsx");
    mountManuscripts();
  });
  await page.getByRole("button", { name: /Evidence and discovery/ }).click();
  const editor = page.getByRole("textbox", { name: "LaTeX source main.tex" });
  await editor.waitFor();
  await page.getByRole("button", { name: "编译", exact: true }).click();
  await page.getByText("编译中", { exact: true }).waitFor();
  assert.ok(
    await page
      .getByRole("button", { name: "Back to manuscripts" })
      .isDisabled(),
  );
  compileRelease();
  await page.waitForFunction(
    () =>
      document.querySelector('canvas[aria-label="PDF page 1"]')?.dataset
        .rendered === "true",
  );
  mkdirSync("test-results", { recursive: true });
  await page.screenshot({ path: "test-results/manuscripts-workspace.png" });
  await editor.focus();
  await page.keyboard.press("Control+End");
  await page.keyboard.type("% Local edit");
  await page.getByRole("button", { name: /审阅/ }).click();
  assert.ok(
    await page
      .getByRole("button", { name: "接受修改", exact: true })
      .isDisabled(),
  );
  conflictNext = true;
  await page.getByRole("button", { name: "保存", exact: true }).click();
  await page.getByText("版本冲突 · main.tex", { exact: true }).waitFor();
  assert.ok(
    (await page.locator(".manuscript-source .view-lines").innerText())
      .replaceAll("\u00a0", " ")
      .includes("Local edit"),
  );
  await page
    .getByRole("button", { name: "已对照，保留草稿并更新基线" })
    .click();
  await page.getByRole("button", { name: "保存", exact: true }).click();
  await page.getByText("所有修改已保存", { exact: true }).waitFor();
  assert.ok(file.content.includes("Local edit"));
  assert.equal(
    requests.filter((request) => request.path.endsWith("/files")).at(-1).body
      .expected_revision,
    2,
  );
  proposal.base_revision = file.revision;
  proposal.original = file.content;
  proposal.proposed = file.content + "\n% Accepted agent contribution";
  await page.getByRole("button", { name: "Refresh proposals" }).click();
  await page.getByRole("button", { name: "接受修改", exact: true }).click();
  await page.getByText("已接受", { exact: true }).waitFor();
  await page.waitForFunction(() =>
    document
      .querySelector(".manuscript-source .view-lines")
      ?.textContent.replaceAll("\u00a0", " ")
      .includes("Accepted agent contribution"),
  );
  await page.getByRole("button", { name: "Comments", exact: true }).click();
  await page
    .getByRole("textbox", { name: "Comment text" })
    .fill("Check the evidence for this claim.");
  await page.getByRole("button", { name: "添加批注", exact: true }).click();
  await page
    .getByText("Check the evidence for this claim.", { exact: true })
    .waitFor();
  await page.getByRole("button", { name: "标为已解决" }).waitFor();
  assert.equal(comments[0].expected_revision, file.revision);
  await page.getByRole("button", { name: "标为已解决" }).click();
  await page.getByRole("button", { name: "重新打开" }).waitFor();
  await page.getByRole("button", { name: "History", exact: true }).click();
  await page.getByRole("button", { name: "恢复此版本" }).click();
  await page.waitForFunction(() =>
    document
      .querySelector(".manuscript-source .view-lines")
      ?.textContent.replaceAll("\u00a0", " ")
      .includes("Describe the evidence."),
  );
  assert.equal(file.content, initial);
  await page.getByRole("button", { name: /文献库与引用/ }).click();
  await page
    .getByRole("button", { name: /Collaborative scientific writing/ })
    .click();
  await page.waitForFunction(() =>
    document
      .querySelector(".manuscript-source .view-lines")
      ?.textContent.replaceAll("\u00a0", " ")
      .includes("\\cite{smith2026}"),
  );
  assert.deepEqual(project.article_ids, ["article1"]);
  await page.getByRole("button", { name: "Back to manuscripts" }).click();
  await page
    .getByText("请先保存本地编辑，再返回项目列表。", { exact: true })
    .waitFor();
  await page.locator("input[type=file]").setInputFiles({
    name: "result.png",
    mimeType: "image/png",
    buffer: Buffer.from("test image fixture"),
  });
  await page
    .getByRole("button", { name: "Delete figures/result.png" })
    .waitFor();
  await page.getByRole("button", { name: "Delete figures/result.png" }).click();
  await page.waitForFunction(
    () => !document.querySelector('[aria-label="Delete figures/result.png"]'),
  );
  proposal.status = "pending";
  proposal.summary = "Incoming agent review";
  await page.getByRole("button", { name: /审阅/ }).click();
  await page
    .getByText("Incoming agent review", { exact: true })
    .waitFor({ timeout: 15000 });
  assert.ok(
    await page
      .getByRole("button", { name: "接受修改", exact: true })
      .isDisabled(),
  );
  await page.screenshot({ path: "test-results/manuscripts-review.png" });
  await page.getByRole("button", { name: "拒绝", exact: true }).click();
  await page.getByText("已拒绝", { exact: true }).waitFor();
  const download = page.waitForEvent("download");
  await page.getByRole("button", { name: "Download source ZIP" }).click();
  assert.equal((await download).suggestedFilename(), "manuscript-source.zip");
  assert.ok(file.content.includes("\\cite{smith2026}"));
  await page.getByRole("button", { name: "Back to manuscripts" }).click();
  await page
    .getByRole("textbox", { name: "Manuscript title" })
    .fill("A new manuscript");
  await page.getByRole("button", { name: "新建 LaTeX 手稿" }).click();
  await page.getByRole("textbox", { name: "LaTeX source main.tex" }).waitFor();
  assert.equal(
    requests.find(
      (request) =>
        request.path === "/api/v1/bib/manuscripts" && request.method === "POST",
    ).body.title,
    "A new manuscript",
  );
  assert.deepEqual(errors, []);
  console.log(
    "PASS manuscripts mocked-HTTP UI: Monaco, real PDF rendering, compile navigation guard, CAS conflict preservation, proposal dirty guard/accept refresh, anchored comments, restore, citation insertion, asset upload/delete",
  );
} finally {
  await browser?.close();
  server.kill();
  if (server.exitCode === null) await once(server, "exit");
}
