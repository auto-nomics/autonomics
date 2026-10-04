const backend = process.env.AUTONOMICS_API_BACKEND ?? "http://127.0.0.1:8765";
const root = new URL("..", import.meta.url).pathname;

function contentType(path: string): string {
  if (path.endsWith(".html")) return "text/html; charset=utf-8";
  if (path.endsWith(".css")) return "text/css; charset=utf-8";
  if (path.endsWith(".js")) return "text/javascript; charset=utf-8";
  return "application/octet-stream";
}

const server = Bun.serve({
  port: Number(process.env.FRONTEND_PORT ?? 5173),
  async fetch(request): Promise<Response> {
    const url = new URL(request.url);

    if (url.pathname.startsWith("/api/")) {
      const backendUrl = `${backend}${url.pathname}${url.search}`;
      const body = request.method === "GET" || request.method === "HEAD"
        ? undefined
        : await request.arrayBuffer();
      return fetch(backendUrl, {
        method: request.method,
        headers: request.headers,
        body,
      });
    }

    const relative = url.pathname === "/" ? "dist/index.html" : `dist${url.pathname}`;
    const file = Bun.file(`${root}/${relative}`);
    return file.exists().then((exists) =>
      exists
        ? new Response(file, { headers: { "content-type": contentType(relative) } })
        : new Response("Not found", { status: 404 }),
    );
  },
});

console.log(`Bun frontend: http://127.0.0.1:${server.port}`);
console.log(`Rust API backend: ${backend}`);
