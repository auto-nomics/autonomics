export type GatewayProbe = {
  running: boolean;
  addr: string;
  /** Whether the caller holds a usable credential. */
  authenticated: boolean;
};

import type { GatewayFrame } from "./types";

export type GatewayFrameHandler = (frame: GatewayFrame) => void;

export type GatewayStatusHandler = (
  status: "connected" | "disconnected" | "reconnected",
) => void;

const isTauri =
  typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;
const WEB_API_PREFIX = "/api/v1";
/** Session endpoints live outside the `/api/v1` namespace. */
const WEB_SESSION_PATH = "/api/session";
let webEventsStarted = false;

declare global {
  interface Window {
    __TAURI_INTERNALS__?: unknown;
  }
}

/**
 * An HTTP failure that carries the response status.
 *
 * The SPA needs the status to tell "not authenticated" (401 — show the
 * login form) apart from a genuine error.
 */
export class GatewayHttpError extends Error {
  readonly status: number;

  constructor(status: number, message: string) {
    super(message);
    this.name = "GatewayHttpError";
    this.status = status;
  }
}

function isNotFound(error: unknown): boolean {
  if (error instanceof GatewayHttpError) return error.status === 404;
  return String(error).includes("404");
}

function normalizeApiPath(path: string): string {
  const trimmed = path.startsWith("/") ? path : `/${path}`;
  const queryStart = trimmed.indexOf("?");
  const pathname = queryStart < 0 ? trimmed : trimmed.slice(0, queryStart);
  const query = queryStart < 0 ? "" : trimmed.slice(queryStart);
  const agents = pathname.match(
    /^\/agents\/(.+?)((?:\/(?:messages|cancel|compact|config|dag|model|sessions))(?:\/.*)?)$/,
  );
  if (!agents) return trimmed;
  const agentPath = agents[1].includes("%")
    ? agents[1]
    : encodeURIComponent(agents[1]);
  return `/agents/${agentPath}${agents[2]}${query}`;
}

function webApiPath(path: string): string {
  return `${WEB_API_PREFIX}${normalizeApiPath(path)}`;
}

async function tauriInvoke<T>(command: string, args?: unknown): Promise<T> {
  const core = await import("@tauri-apps/api/core");
  return core.invoke<T>(command, args as any);
}

export async function gatewayRequest(
  method: string,
  path: string,
  body?: unknown,
): Promise<any> {
  if (isTauri) {
    const result = await tauriInvoke<{ data: any }>("api", {
      method,
      path,
      body: body === undefined ? null : body,
    });
    return result.data;
  }

  // The session cookie rides along automatically; the bearer token
  // never reaches the browser.
  const response = await fetch(webApiPath(path), {
    method,
    credentials: "same-origin",
    headers: body === undefined ? {} : { "Content-Type": "application/json" },
    body: body === undefined ? undefined : JSON.stringify(body),
  });
  const text = await response.text();
  const payload = text ? JSON.parse(text) : null;
  if (!response.ok) {
    throw new GatewayHttpError(
      response.status,
      payload?.error ?? `${response.status} ${response.statusText}`,
    );
  }
  return payload;
}

export async function gatewayRequestOptional(path: string): Promise<any | null> {
  try {
    return await gatewayRequest("GET", path);
  } catch (error) {
    if (isNotFound(error)) return null;
    throw error;
  }
}

/**
 * Exchange the gateway bearer token for a browser session cookie.
 *
 * The token is posted once and never stored in JavaScript: the server
 * answers with an `HttpOnly` cookie the browser sends on its own.
 */
export async function loginGateway(token: string): Promise<void> {
  if (isTauri) {
    // The desktop app reads the token from the local state directory
    // and attaches it in Rust — there is nothing to exchange.
    return;
  }
  const response = await fetch(WEB_SESSION_PATH, {
    method: "POST",
    credentials: "same-origin",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({ token }),
  });
  if (!response.ok) {
    const payload = await response.json().catch(() => null);
    throw new GatewayHttpError(
      response.status,
      payload?.error ?? `登录失败 (${response.status})`,
    );
  }
}

/** Drop the browser session. */
export async function logoutGateway(): Promise<void> {
  if (isTauri) return;
  await fetch(WEB_SESSION_PATH, {
    method: "DELETE",
    credentials: "same-origin",
  }).catch(() => {});
}

/** Whether the browser holds a live session cookie. */
export async function sessionAuthenticated(): Promise<boolean> {
  if (isTauri) {
    const view = await tauriInvoke<{ token_found: boolean }>("probe");
    return Boolean(view.token_found);
  }
  const response = await fetch(WEB_SESSION_PATH, {
    credentials: "same-origin",
  }).catch(() => null);
  return Boolean(response?.ok);
}

export async function probeGateway(): Promise<GatewayProbe> {
  if (isTauri) {
    const view = await tauriInvoke<{
      running: boolean;
      addr: string;
      token_found: boolean;
    }>("probe");
    return {
      running: view.running,
      addr: view.addr,
      authenticated: Boolean(view.token_found),
    };
  }
  return {
    running: true,
    addr: window.location.host,
    authenticated: await sessionAuthenticated(),
  };
}

export async function startGatewayEvents(
  lastSeq: number,
  handlers: {
    onFrame: GatewayFrameHandler;
    onStatus: GatewayStatusHandler;
  },
): Promise<boolean> {
  if (isTauri) {
    const events = await import("@tauri-apps/api/event");
    await Promise.all([
      events.listen<GatewayFrame>(
        "gateway-frame",
        (event) => handlers.onFrame(event.payload),
      ),
      events.listen<"disconnected" | "reconnected">(
        "gateway-status",
        (event) => handlers.onStatus(event.payload),
      ),
    ]);
    return tauriInvoke("start_events", { lastSeq });
  }

  if (!webEventsStarted) {
    webEventsStarted = true;
    void connectEvents(lastSeq, handlers);
  }
  return true;
}

async function connectEvents(
  lastSeq: number,
  handlers: {
    onFrame: GatewayFrameHandler;
    onStatus: GatewayStatusHandler;
  },
): Promise<void> {
  let sequence = lastSeq;
  let disconnected = false;
  for (;;) {
    try {
      const response = await fetch(webApiPath("/events"), {
        credentials: "same-origin",
        headers: {
          "Last-Event-ID": String(sequence),
          Accept: "text/event-stream",
        },
      });
      if (!response.ok || !response.body) throw new Error(`SSE ${response.status}`);
      if (disconnected) {
        disconnected = false;
        handlers.onStatus("reconnected");
      }

      const reader = response.body.getReader();
      const decoder = new TextDecoder();
      let buffered = "";
      for (;;) {
        const { value, done } = await reader.read();
        if (done) break;
        buffered += decoder.decode(value, { stream: true });
        for (;;) {
          const boundary = buffered.search(/\r?\n\r?\n/);
          if (boundary < 0) break;
          const leading = buffered.match(/^\r?\n/);
          const skip = leading ? leading[0].length : 2;
          const rawEvent = buffered.slice(0, boundary);
          buffered = buffered.slice(boundary + skip);
          const frame = parseSseEvent(rawEvent);
          if (!frame || frame.seq <= sequence) continue;
          sequence = frame.seq;
          handlers.onFrame(frame);
        }
      }
    } catch {
      // Reconnect below.
    }
    if (!disconnected) {
      disconnected = true;
      handlers.onStatus("disconnected");
    }
    await new Promise((resolve) => setTimeout(resolve, 500));
  }
}

function parseSseEvent(raw: string): {
  seq: number;
  kind: GatewayFrame["kind"];
  data: any;
} | null {
  let id: number | null = null;
  let kind: string | null = null;
  let data = "";
  for (const line of raw.split(/\r?\n/)) {
    if (line.startsWith("id:")) id = Number(line.slice(3).trim());
    else if (line.startsWith("event:")) kind = line.slice(6).trim();
    else if (line.startsWith("data:")) {
      const value = line.slice(5).trimStart();
      data += data ? `\n${value}` : value;
    }
  }
  if (id === null || !data) return null;
  if (kind !== "agent" && kind !== "host" && kind !== "notice" && kind !== "lag") {
    return null;
  }
  try {
    return { seq: id, kind, data: JSON.parse(data) };
  } catch {
    return null;
  }
}

export async function pickUploadFile(): Promise<File | string | null> {
  if (!isTauri) {
    return new Promise<File | null>((resolve) => {
      const input = document.createElement("input");
      input.type = "file";
      input.accept = ".pdf,.html,.htm,.txt,application/pdf,text/html,text/plain";
      input.onchange = () => resolve(input.files?.[0] ?? null);
      input.oncancel = () => resolve(null);
      input.click();
    });
  }
  const dialog = await import("@tauri-apps/plugin-dialog");
  return dialog.open({
    multiple: false,
    directory: false,
    title: "Select full text",
    filters: [
      { name: "Documents", extensions: ["pdf", "html", "htm", "txt"] },
      { name: "All files", extensions: ["*"] },
    ],
  });
}

export async function uploadBibFulltext(
  articleId: string,
  file: File | string,
): Promise<any> {
  const path = normalizeApiPath(
    `/bib/articles/${encodeURIComponent(articleId)}/fulltext`,
  );
  if (isTauri) {
    return tauriInvoke("upload_bib_fulltext", {
      request: { articleId, path: file },
    });
  }
  if (!(file instanceof File)) throw new Error("A browser file is required");
  const form = new FormData();
  form.append("file", file, file.name);
  const response = await fetch(webApiPath(path), {
    method: "POST",
    credentials: "same-origin",
    body: form,
  });
  const text = await response.text();
  const payload = text ? JSON.parse(text) : null;
  if (!response.ok) throw new Error(payload?.error ?? `Upload ${response.status}`);
  return payload;
}

export async function fetchBibFulltextRaw(
  articleId: string,
): Promise<{ bytes: Uint8Array; contentType: string }> {
  if (isTauri) {
    const result = await tauriInvoke<{
      data_base64: string;
      content_type: string;
    }>("bib_fulltext_raw", { articleId });
    const binary = atob(result.data_base64);
    const bytes = new Uint8Array(binary.length);
    for (let index = 0; index < binary.length; index += 1) {
      bytes[index] = binary.charCodeAt(index);
    }
    return { bytes, contentType: result.content_type };
  }

  const response = await fetch(
    webApiPath(`/bib/articles/${encodeURIComponent(articleId)}/fulltext/raw`),
    { credentials: "same-origin" },
  );
  if (!response.ok) {
    const payload = await response.json().catch(() => null);
    throw new Error(payload?.error ?? `Unable to load PDF (${response.status})`);
  }
  return {
    bytes: new Uint8Array(await response.arrayBuffer()),
    contentType: response.headers.get("content-type") ?? "application/pdf",
  };
}

export async function saveTextFile(
  defaultPath: string,
  contents: string,
): Promise<string | null> {
  if (isTauri) {
    const dialog = await import("@tauri-apps/plugin-dialog");
    const path = await dialog.save({
      title: "Save export",
      defaultPath,
    });
    if (!path) return null;
    await tauriInvoke("save_bib_export", {
      request: { path, contents },
    });
    return path;
  }

  const url = URL.createObjectURL(new Blob([contents], { type: "text/plain" }));
  const anchor = document.createElement("a");
  anchor.href = url;
  anchor.download = defaultPath;
  anchor.click();
  URL.revokeObjectURL(url);
  return defaultPath;
}

export function reportFrontendReady(
  stage: string,
  detail: Record<string, unknown>,
): void {
  if (!isTauri) return;
  void tauriInvoke("frontend_ready", { stage, detail }).catch(() => {});
}

// JSON long polling uses the same authenticated bridge in web and Tauri.
export function watchBibCollections(onChange: () => void): () => void {
  let stopped = false;
  let revision: number | undefined;
  void (async () => {
    while (!stopped) {
      try {
        const response = await gatewayRequest("GET", `/bib/collections/changes${revision === undefined ? "" : `?since=${revision}`}`);
        if (stopped) return;
        if (typeof response.revision !== "number") throw new Error("Missing bibliography revision");
        if (revision !== undefined && revision !== response.revision) onChange();
        revision = response.revision;
      } catch { if (!stopped) await new Promise(resolve => setTimeout(resolve, 2000)); }
    }
  })();
  return () => { stopped = true; };
}
