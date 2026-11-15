// Full application shell with mocked HTTP: manuscripts/library focus and sidebar restoration.
import { chromium } from "playwright";
import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import { once } from "node:events";
import { existsSync, mkdirSync } from "node:fs";
const origin = "http://127.0.0.1:1444";
const server = spawn(
  process.execPath,
  [
    "node_modules/vite/bin/vite.js",
    "--host",
    "127.0.0.1",
    "--port",
    "1444",
    "--strictPort",
  ],
  { stdio: ["ignore", "pipe", "pipe"] },
);
let remoteRevision = 1;
let savedRevision;
let output = "",
  browser;
server.stdout.on("data", (value) => (output += value));
server.stderr.on("data", (value) => (output += value));
try {
  for (let attempt = 0; !output.includes(origin); attempt++) {
    if (attempt > 1200 || server.exitCode !== null) throw new Error(output);
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
  page.setDefaultTimeout(30000);
  const errors = [];
  page.on("pageerror", (error) => {
    errors.push(error.message);
    console.error(error.stack);
  });
  await page.route(`${origin}/api/session`, (route) =>
    route.fulfill({ json: {} }),
  );
  await page.route("**/api/v1/**", (route) => {
    const path = new URL(route.request().url()).pathname.replace("/api/v1", "");
    if (path === "/state")
      return route.fulfill({
        json: { last_seq: 0, agents: [], contexts: {}, display_settings: {} },
      });
    if (path === "/bib/manuscripts")
      return route.fulfill({
        json: [
          {
            id: "p1",
            title: "A research manuscript",
            main_file: "main.tex",
            article_ids: [],
            created_at: "2026-11-15",
            updated_at: "2026-11-15",
          },
        ],
      });
    if (path === "/bib/manuscripts/p1")
      return route.fulfill({
        json: {
          project: {
            id: "p1",
            title: "A research manuscript",
            main_file: "main.tex",
            article_ids: [],
            created_at: "2026-11-15",
            updated_at: "2026-11-15",
          },
          files: [
            {
              path: "main.tex",
              content:
                "\\documentclass{article}\n\\begin{document}\n\\section{Introduction}\nEvidence and discovery.\n\\end{document}",
              revision: remoteRevision,
            },
          ],
          assets: [],
          proposals: [],
          comments: [],
        },
      });
    if (path.endsWith("/files") && route.request().method() === "PUT") {
      savedRevision = route.request().postDataJSON().expected_revision;
      return route.fulfill({
        status: 409,
        json: { error: "Revision conflict" },
      });
    }
    if (path.startsWith("/bib/manuscripts/"))
      return route.fulfill({ status: 404, json: { error: "No build" } });
    if (path.startsWith("/bib/"))
      return route.fulfill({
        json: {
          collections: [],
          articles: [],
          sources: [],
          rules: [],
          items: [],
          snapshots: [],
          total: 0,
        },
      });
    if (path === "/model-config")
      return route.fulfill({ json: { models: [], providers: [] } });
    if (path === "/storage/agents" || path === "/profiles")
      return route.fulfill({ json: [] });
    if (path === "/settings") return route.fulfill({ json: {} });
    return route.fulfill({
      status: 404,
      json: { error: `Not mocked ${path}` },
    });
  });
  await page.route("**/api/v1/events", () => new Promise(() => {}));
  await page.goto(origin, { waitUntil: "domcontentloaded" });
  await page.locator("main[data-workbench]").waitFor();
  const reservedPaths = await page.evaluate(async () => {
    const { readDrafts, getDraft } =
      await import("/src/features/bibliography/manuscripts/draft-recovery.ts");
    const key = "manuscript-test-reserved-paths";
    sessionStorage.setItem(
      key,
      JSON.stringify(
        Object.fromEntries(
          ["__proto__", "constructor"].map((path) => [
            path,
            {
              content: "local",
              base: { path, content: "base", revision: 1, updated_at: "" },
            },
          ]),
        ),
      ),
    );
    const drafts = readDrafts(key);
    sessionStorage.removeItem(key);
    return {
      keys: Object.keys(drafts),
      prototype: Object.getPrototypeOf(drafts),
      missing: getDraft({}, "constructor"),
      contents: [
        getDraft(drafts, "__proto__")?.content,
        getDraft(drafts, "constructor")?.content,
      ],
    };
  });
  assert.deepEqual(reservedPaths, {
    keys: ["__proto__", "constructor"],
    prototype: null,
    missing: undefined,
    contents: ["local", "local"],
  });
  const rail = page.getByRole("navigation", { name: "工作台活动栏" });
  await rail.getByRole("button", { name: "Bibliography", exact: true }).click();
  await page
    .locator("aside[data-slot=workbench-sidebar]")
    .getByRole("tree", { name: "Collections" })
    .waitFor();
  await rail.getByRole("button", { name: "Manuscripts", exact: true }).click();
  await page.getByRole("tab", { name: "LaTeX 手稿", exact: true }).waitFor();
  await page.getByRole("textbox", { name: "Manuscript title" }).waitFor();
  assert.equal(
    await rail
      .getByRole("button", { name: "Manuscripts", exact: true })
      .getAttribute("aria-current"),
    "page",
  );
  await page
    .locator("aside[data-slot=workbench-sidebar]")
    .waitFor({ state: "hidden" });
  await page.getByRole("button", { name: /A research manuscript/ }).click();
  await page.getByRole("textbox", { name: "LaTeX source main.tex" }).waitFor();
  mkdirSync("test-results", { recursive: true });
  await page.screenshot({ path: "test-results/manuscripts-shell.png" });
  await rail.getByRole("button", { name: "Bibliography", exact: true }).click();
  await page
    .locator("aside[data-slot=workbench-sidebar]")
    .getByRole("tree", { name: "Collections" })
    .waitFor();
  assert.equal(
    await rail
      .getByRole("button", { name: "Bibliography", exact: true })
      .getAttribute("aria-current"),
    "page",
  );
  await rail.getByRole("button", { name: "Manuscripts", exact: true }).click();
  await page.getByRole("textbox", { name: "LaTeX source main.tex" }).waitFor();
  await page
    .locator("aside[data-slot=workbench-sidebar]")
    .waitFor({ state: "hidden" });
  await page.getByRole("textbox", { name: "LaTeX source main.tex" }).focus();
  await page.keyboard.press("Control+End");
  await page.keyboard.type("\n% Recover me after close");
  await page.waitForFunction(() =>
    Object.keys(sessionStorage).some(
      (key) =>
        key.startsWith("autonomics.manuscript-drafts.v1:") &&
        sessionStorage.getItem(key)?.includes("Recover me"),
    ),
  );
  await page
    .getByRole("tab", { name: "LaTeX 手稿", exact: true })
    .getByRole("button", { name: "关闭 LaTeX 手稿" })
    .click();
  await page.locator("aside[data-slot=workbench-sidebar]").waitFor();
  remoteRevision = 2;
  await rail.getByRole("button", { name: "Manuscripts", exact: true }).click();
  await page.getByRole("button", { name: /A research manuscript/ }).click();
  await page
    .getByText(
      "已恢复此项目的本地草稿。保存时将校验原版本，避免覆盖其他协作者的修改。",
      { exact: true },
    )
    .waitFor();
  await page.getByRole("button", { name: "保存", exact: true }).click();
  await page.getByText("版本冲突 · main.tex", { exact: true }).waitFor();
  assert.equal(
    savedRevision,
    1,
    "Recovered draft must save against original base revision, never remote revision",
  );
  assert.ok(
    (await page.locator(".manuscript-source .view-lines").innerText())
      .replaceAll("\u00a0", " ")
      .includes("Recover me"),
  );
  await page.evaluate(() => {
    window.originalStorageSetItem = Storage.prototype.setItem;
    Storage.prototype.setItem = function () {
      throw new DOMException("Storage quota exceeded", "QuotaExceededError");
    };
  });
  await page.getByRole("textbox", { name: "LaTeX source main.tex" }).focus();
  await page.keyboard.press("Control+End");
  await page.keyboard.type("\n% Live draft survives quota failure");
  await page.getByText(/本地草稿备份失败/).waitFor();
  assert.ok(
    (await page.locator(".manuscript-source .view-lines").innerText())
      .replaceAll("\u00a0", " ")
      .includes("Live draft survives"),
  );
  await page.evaluate(() => {
    Storage.prototype.setItem = window.originalStorageSetItem;
  });
  assert.deepEqual(errors, []);
  console.log(
    "PASS manuscripts full shell mocked-HTTP: rail active state, independent editor tab, local file tree, automatic sidebar collapse/restore, switching to library, close tab restoration, persisted draft recovery and stale CAS",
  );
} finally {
  await browser?.close();
  server.kill();
  if (server.exitCode === null) await once(server, "exit");
}
