/** VFS REST client over gatewayRequest — mirrors `/api/v1/vfs`.
 *
 * Binary transfers bypass `gatewayRequest` (JSON-only): the web build
 * fetches raw streams with the session cookie; the Tauri build goes
 * through dedicated invoke commands (`gateway.ts` pattern of
 * `fetchBibFulltextRaw`/`uploadBibFulltext`).
 */
import { gatewayRequest } from "@/gateway";
import { isTauri } from "@/lib/is-tauri";

export interface VfsEntry {
  /** Full virtual path, e.g. `/data/ldsc/panel.parquet`. */
  path: string;
  /** Final path segment. */
  name: string;
  is_dir: boolean;
  size: number;
  last_modified?: string | null;
}

export interface VfsListing {
  path: string;
  entries: VfsEntry[];
  returned: number;
  truncated: boolean;
  next_offset?: number | null;
}

export interface VfsMount {
  path: string;
  backend: string;
  source: string;
  read_only: boolean;
}

export interface VfsStat {
  path: string;
  name: string;
  is_dir: boolean;
  size: number;
  last_modified?: string | null;
  read_only: boolean;
  mount?: VfsMount | null;
}

export interface VfsTextRead {
  path: string;
  contents: string;
  size: number;
  truncated: boolean;
}

/** gatewayRequest returns untyped payloads; narrow here. */
async function get<T>(path: string): Promise<T> {
  return (await gatewayRequest("GET", path)) as T;
}

function query(params: Record<string, string | number | boolean | undefined>): string {
  const search = new URLSearchParams();
  for (const [key, value] of Object.entries(params)) {
    if (value !== undefined) search.set(key, String(value));
  }
  const encoded = search.toString();
  return encoded ? `?${encoded}` : "";
}

export async function listDir(
  path: string,
  options: { recursive?: boolean; limit?: number; offset?: number } = {},
): Promise<VfsListing> {
  return get<VfsListing>(`/vfs/list${query({ path, ...options })}`);
}

/** List a directory fully, following `next_offset` until exhausted. */
export async function listDirAll(path: string, limit = 500): Promise<VfsEntry[]> {
  const entries: VfsEntry[] = [];
  let offset: number | undefined;
  for (;;) {
    const page = await listDir(path, { limit, offset });
    entries.push(...page.entries);
    if (!page.truncated || page.next_offset == null) return entries;
    offset = page.next_offset;
  }
}

export async function statPath(path: string): Promise<VfsStat> {
  return get<VfsStat>(`/vfs/stat${query({ path })}`);
}

export async function listMounts(): Promise<{ mounts: VfsMount[]; count: number }> {
  return get<{ mounts: VfsMount[]; count: number }>("/vfs/mounts");
}

export async function readTextFile(path: string, maxBytes?: number): Promise<VfsTextRead> {
  return get<VfsTextRead>(`/vfs/text${query({ path, max_bytes: maxBytes })}`);
}

/** Raw-file URL for same-origin `fetch`/`<img src>` (session cookie auth). */
export function vfsFileUrl(path: string, download = false): string {
  return `/api/v1/vfs/file${query({ path, download })}`;
}

export async function mkdir(path: string): Promise<void> {
  await gatewayRequest("POST", "/vfs/mkdir", { path });
}

export async function removeEntry(path: string, recursive = false): Promise<void> {
  await gatewayRequest("DELETE", `/vfs/entry${query({ path, recursive })}`);
}

export async function moveEntry(src: string, dst: string): Promise<void> {
  await gatewayRequest("POST", "/vfs/move", { src, dst });
}

/** Conflict-checked editor save; `original: null` overwrites. */
export async function saveTextFile(path: string, contents: string, original: string | null): Promise<void> {
  await gatewayRequest("PUT", "/vfs/text", { path, contents, original });
}

/** Upload a browser `File` as raw bytes (web build). */
export async function uploadBrowserFile(path: string, file: File): Promise<void> {
  const response = await fetch(vfsFileUrl(path), {
    method: "PUT",
    credentials: "same-origin",
    body: file,
  });
  if (!response.ok) {
    const payload = await response.json().catch(() => null);
    throw new Error(payload?.error ?? `上传失败 (${response.status})`);
  }
}

/** Pick a local file for upload: browser file input or Tauri dialog. */
export async function pickUploadSource(): Promise<File | string | null> {
  if (isTauri) {
    const dialog = await import("@tauri-apps/plugin-dialog");
    return dialog.open({ multiple: false, directory: false, title: "上传到 VFS" });
  }
  return new Promise<File | null>((resolve) => {
    const input = document.createElement("input");
    input.type = "file";
    input.onchange = () => resolve(input.files?.[0] ?? null);
    input.oncancel = () => resolve(null);
    input.click();
  });
}

/** Upload the picked source into `dir`, returning the written path. */
export async function uploadToDir(dir: string, source: File | string): Promise<string> {
  const name = source instanceof File
    ? source.name
    : source.split("/").filter(Boolean).pop() ?? "upload";
  const path = dir === "/" ? `/${name}` : `${dir.replace(/\/+$/, "")}/${name}`;
  if (isTauri) {
    if (typeof source !== "string") throw new Error("桌面端上传需要一个本地文件路径");
    const { invoke } = await import("@tauri-apps/api/core");
    await invoke("vfs_upload", { request: { path, localPath: source } });
    return path;
  }
  if (!(source instanceof File)) throw new Error("浏览器上传需要一个文件");
  await uploadBrowserFile(path, source);
  return path;
}

/** Download a VFS file to the local machine. */
export async function downloadVfsFile(path: string): Promise<void> {
  const name = path.split("/").filter(Boolean).pop() ?? "file";
  if (isTauri) {
    const { invoke } = await import("@tauri-apps/api/core");
    const result = await invoke<{ data_base64: string }>("vfs_download_raw", { request: { path } });
    const binary = atob(result.data_base64);
    const bytes = new Uint8Array(binary.length);
    for (let index = 0; index < binary.length; index += 1) bytes[index] = binary.charCodeAt(index);
    const url = URL.createObjectURL(new Blob([bytes], { type: "application/octet-stream" }));
    const anchor = document.createElement("a");
    anchor.href = url;
    anchor.download = name;
    anchor.click();
    URL.revokeObjectURL(url);
    return;
  }
  const response = await fetch(vfsFileUrl(path, true), { credentials: "same-origin" });
  if (!response.ok) {
    const payload = await response.json().catch(() => null);
    throw new Error(payload?.error ?? `下载失败 (${response.status})`);
  }
  const url = URL.createObjectURL(await response.blob());
  const anchor = document.createElement("a");
  anchor.href = url;
  anchor.download = name;
  anchor.click();
  URL.revokeObjectURL(url);
}

const TEXT_EXTENSIONS = new Set([
  "txt", "md", "markdown", "log", "csv", "tsv", "json", "yaml", "yml", "toml", "xml",
  "html", "htm", "css", "js", "mjs", "ts", "tsx", "jsx", "py", "r", "rs", "go", "java",
  "c", "h", "cpp", "hpp", "sh", "bash", "zsh", "sql", "env", "gitignore", "dockerfile",
  "conf", "ini", "cfg", "diff", "patch", "svg",
]);

const IMAGE_EXTENSIONS = new Set(["png", "jpg", "jpeg", "gif", "webp", "avif", "ico", "bmp"]);

function extensionOf(path: string): string {
  const name = path.split("/").filter(Boolean).pop() ?? "";
  const dot = name.lastIndexOf(".");
  // dotfiles like `.gitignore` count as extension-less text
  if (dot <= 0) return name.startsWith(".") ? name.slice(1).toLowerCase() : "";
  return name.slice(dot + 1).toLowerCase();
}

export function isTextPath(path: string): boolean {
  const ext = extensionOf(path);
  return ext === "" || TEXT_EXTENSIONS.has(ext);
}

export function isImagePath(path: string): boolean {
  return IMAGE_EXTENSIONS.has(extensionOf(path));
}

export function formatBytes(size: number): string {
  if (size < 1024) return `${size} B`;
  if (size < 1024 * 1024) return `${(size / 1024).toFixed(1)} KB`;
  if (size < 1024 * 1024 * 1024) return `${(size / 1024 / 1024).toFixed(1)} MB`;
  return `${(size / 1024 / 1024 / 1024).toFixed(1)} GB`;
}
