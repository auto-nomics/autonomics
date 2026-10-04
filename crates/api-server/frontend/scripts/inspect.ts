import { chromium } from "playwright";

interface MockArticle {
  id: string;
  title: string;
  authors: Array<{ last_name: string; fore_name?: string }>;
  identifiers: Array<{ kind: string; value: string }>;
  abstract_text?: string;
  year?: number;
  journal?: string;
  source?: string;
}

const articles: MockArticle[] = [
  {
    id: "doi:10.1000/genetics",
    title: "Polygenic architecture of complex traits across ancestries: a multi-ancestry GWAS of cardiovascular disease risk using whole-genome sequencing data from the All of Us cohort",
    authors: [{ last_name: "Smith", fore_name: "John" }, { last_name: "Chen", fore_name: "Wei" }],
    identifiers: [{ kind: "doi", value: "10.1000/genetics" }],
    abstract_text:
      "This record exercises the collection-driven item list, metadata panel, and grouped All Records view with a realistic title and author line.",
    year: 2026,
    journal: "Nature Genetics",
    source: "manual",
  },
  {
    id: "pmid:30124452",
    title: "Open-access full text retrieval for biomedical literature workflows",
    authors: [{ last_name: "Garcia", fore_name: "Ana" }],
    identifiers: [{ kind: "pmid", value: "30124452" }],
    abstract_text: "An unfiled record used to verify the virtual Unfiled collection.",
    year: 2025,
    journal: "Bioinformatics",
    source: "pubmed",
  },
];

const collections = [
  {
    id: "col-genetics",
    name: "Genetics methods — Polygenic architecture across ancestries",
    article_ids: ["doi:10.1000/genetics"],
    status: "active",
  },
  { id: "col-writing", name: "Writing", article_ids: [], status: "active" },
];

function json(data: unknown, status = 200): Response {
  return new Response(JSON.stringify(data), {
    status,
    headers: { "content-type": "application/json" },
  });
}

function detail(id: string): unknown {
  const article = articles.find((item) => item.id === id);
  return {
    article,
    fulltext: article?.id === "doi:10.1000/genetics"
      ? {
          article_id: article.id,
          file_path: "vfs:///literature/doi%3A10.1000%2Fgenetics/test.pdf",
          file_format: "pdf",
          text_content:
            "Extracted full text exercises the scrollable reader surface without overflowing the metadata layout.",
          source: "user_upload",
          file_hash: "a".repeat(64),
          file_size: 1024,
        }
      : null,
    annotations: [
      { id: "ann-1", kind: "note", content: "Check collection filtering.", page: 2 },
    ],
  };
}

const root = new URL("..", import.meta.url).pathname;
const server = Bun.serve({
  port: Number(process.env.INSPECT_PORT ?? 5174),
  async fetch(request): Promise<Response> {
    const url = new URL(request.url);
    if (url.pathname.startsWith("/api/v1/bib/health")) {
      return json({ status: "ok", article_count: articles.length, sources: ["pubmed", "arxiv"] });
    }
    if (url.pathname === "/api/v1/bib/collections") {
      return json({ collections });
    }
    if (url.pathname === "/api/v1/bib/articles/unfiled") {
      return json({ total: 1, articles: articles.filter((article) => article.id === "pmid:30124452") });
    }
    if (url.pathname === "/api/v1/bib/articles") {
      return json({
        total: articles.length,
        hits: articles.map((article) => ({
          article_id: article.id,
          title: article.title,
          snippet: article.abstract_text ?? "",
          score: 0,
        })),
        articles,
      });
    }
    const detailMatch = url.pathname.match(/^\/api\/v1\/bib\/articles\/([^/]+)$/);
    if (detailMatch) return json(detail(decodeURIComponent(detailMatch[1])));
    const collectionMatch = url.pathname.match(/^\/api\/v1\/bib\/collections\/([^/]+)\/articles$/);
    if (collectionMatch) {
      const id = decodeURIComponent(collectionMatch[1]);
      const collection = collections.find((item) => item.id === id);
      return json({
        associations: [],
        articles: articles.filter((article) => collection?.article_ids.includes(article.id)),
      });
    }

    const rawMatch = url.pathname.match(/^\/api\/v1\/bib\/articles\/([^/]+)\/fulltext\/raw$/);
    if (rawMatch) {
      const pdfBytes = new Uint8Array([
        0x25,0x50,0x44,0x46,0x2d,0x31,0x2e,0x34,0x0a,0x25,0x9b,0x9c,0x9c,0x9c,0x0a,
        0x31,0x20,0x30,0x20,0x6f,0x62,0x6a,0x0a,0x3c,0x3c,0x20,0x2f,0x54,0x79,0x70,0x65,0x20,
        0x2f,0x43,0x61,0x74,0x61,0x6c,0x6f,0x67,0x20,0x2f,0x50,0x61,0x67,0x65,0x73,0x20,
        0x32,0x20,0x30,0x20,0x52,0x20,0x3e,0x3e,0x0a,0x65,0x6e,0x64,0x6f,0x62,0x6a,0x0a,
        0x78,0x72,0x65,0x66,0x0a,0x30,0x20,0x31,0x0a,0x30,0x30,0x30,0x30,0x30,0x30,0x30,
        0x30,0x30,0x30,0x20,0x36,0x35,0x35,0x33,0x35,0x20,0x66,0x0a,0x30,0x30,0x30,0x30,
        0x30,0x30,0x30,0x30,0x30,0x30,0x20,0x36,0x35,0x35,0x33,0x35,0x20,0x6e,0x0a,0x74,
        0x72,0x61,0x69,0x6c,0x65,0x72,0x0a,0x3c,0x3c,0x20,0x2f,0x53,0x69,0x7a,0x65,0x20,
        0x36,0x35,0x35,0x33,0x35,0x20,0x2f,0x52,0x6f,0x6f,0x74,0x20,0x31,0x20,0x30,0x20,
        0x52,0x20,0x3e,0x3e,0x0a,0x73,0x74,0x61,0x72,0x74,0x78,0x72,0x65,0x66,0x0a,0x39,
        0x38,0x0a,0x25,0x25,0x45,0x4f,0x46,0x0a,
      ]);
      return new Response(pdfBytes, {
        status: 200,
        headers: { "content-type": "application/pdf" },
      });
    }

    const relative = url.pathname === "/" ? "dist/index.html" : `dist${url.pathname}`;
    const file = Bun.file(`${root}/${relative}`);
    return file.exists().then((exists) =>
      exists
        ? new Response(file, {
            headers: {
              "content-type": relative.endsWith(".js")
                ? "text/javascript"
                : relative.endsWith(".css")
                  ? "text/css"
                  : "text/html",
            },
          })
        : new Response("Not found", { status: 404 }),
    );
  },
});

const browser = await chromium.launch({
  executablePath: "/usr/bin/chromium",
  headless: true,
});

async function capture(width: number, height: number, suffix: string): Promise<void> {
  const page = await browser.newPage({ viewport: { width, height } });
  page.on('console', (msg) => console.log('[console]', msg.type(), msg.text()));
  page.on('pageerror', (err) => console.log('[pageerror]', err.message));
  await page.goto(`http://127.0.0.1:${server.port}/`, { waitUntil: "networkidle" });
  await page.getByText("Polygenic architecture of complex").waitFor();
  await page.screenshot({ path: `/tmp/autonomics-bib-${suffix}.png`, fullPage: true });
  await page.close();
}

await capture(1600, 950, "desktop");
await capture(390, 920, "mobile");
await captureWithDetail();
await captureWithPdf();
async function captureWithDetail(): Promise<void> {
  const page = await browser.newPage({ viewport: { width: 1600, height: 950 } });
  await page.goto(`http://127.0.0.1:${server.port}/`, { waitUntil: "networkidle" });
  await page.getByText("Polygenic architecture of complex").waitFor();
  await page.getByText("Polygenic architecture of complex").click();
  await page.getByText("Full text").waitFor();
  await page.screenshot({ path: `/tmp/autonomics-bib-detail.png`, fullPage: true });
  await page.close();
}

async function captureWithPdf(): Promise<void> {
  const page = await browser.newPage({ viewport: { width: 1600, height: 950 } });
  page.on('pageerror', (err) => console.log('[pdf pageerror]', err.message));
  page.on('requestfailed', (req) => console.log('[pdf requestfailed]', req.url(), req.failure()?.errorText));
  try {
    await page.goto(`http://127.0.0.1:${server.port}/`, { waitUntil: "networkidle" });
    await page.getByText("Polygenic architecture of complex").waitFor();
    await page.getByText("Polygenic architecture of complex").click();
    const button = page.getByRole('button', { name: /Read PDF/ });
    await button.waitFor();
    await button.click();
    // Allow the embedpdf bundle and worker to load and the first page to paint.
    await page.waitForLoadState('networkidle', { timeout: 15000 }).catch(() => {});
    await page.waitForTimeout(7000);
    await page.screenshot({ path: `/tmp/autonomics-bib-pdf.png`, fullPage: true });
  } catch (error) {
    console.log('[pdf capture failed]', error instanceof Error ? error.message : String(error));
  } finally {
    await page.close();
  }
}
await browser.close();
server.stop(true);
console.log("Screenshots: /tmp/autonomics-bib-desktop.png, /tmp/autonomics-bib-mobile.png, /tmp/autonomics-bib-detail.png, /tmp/autonomics-bib-pdf.png");
