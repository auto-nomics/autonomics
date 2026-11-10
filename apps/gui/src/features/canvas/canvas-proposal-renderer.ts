import { useEffect, useState } from "react";
import { exportToBlob, FONT_FAMILY, restoreElements } from "@excalidraw/excalidraw";
import type { ExcalidrawInitialDataState } from "@excalidraw/excalidraw/types";
import { gatewayRequest, GatewayHttpError } from "@/gateway";
type Request = { proposal_id: string; base_revision: number; document_sha256: string; document: ExcalidrawInitialDataState };
let fontFingerprint: Promise<string> | undefined;
async function scientificFontHash() {
  fontFingerprint ??= (async () => {
    const base = "/excalidraw/fonts/Liberation/";
    const [manifest, file] = await Promise.all([fetch(`${base}LiberationSans-Regular.sources.json`), fetch(`${base}LiberationSans-Regular.woff2`)]);
    if (!manifest.ok || !file.ok) throw new Error("Scientific font unavailable");
    const source = await manifest.json() as { sha256: string };
    const hash = [...new Uint8Array(await crypto.subtle.digest("SHA-256", await file.arrayBuffer()))].map(v => v.toString(16).padStart(2, "0")).join("");
    if (hash !== source.sha256) throw new Error("Scientific font fingerprint mismatch");
    return hash;
  })().catch(cause => { fontFingerprint = undefined; throw cause; });
  return fontFingerprint;
}
export async function renderCandidatePreview(source: ExcalidrawInitialDataState) {
  const scene = structuredClone(source), elements = restoreElements(scene.elements, null, { repairBindings: true, refreshDimensions: false }), files = scene.files ?? {};
  const texts = elements.filter(e => !e.isDeleted && e.type === "text");
  const family9 = texts.some(e => e.type === "text" && e.fontFamily === 9);
  const fingerprint = family9 ? await scientificFontHash() : null;
  const imageIds = new Set(elements.filter(e => !e.isDeleted && e.type === "image").map(e => e.type === "image" ? e.fileId : null));
  for (const id of imageIds) {
    const url = id && files[id]?.dataURL;
    if (!url || !url.startsWith("data:image/")) throw new Error("Candidate image file missing");
    const image = new Image(); image.src = url; await image.decode();
  }
  const appState = { ...scene.appState, exportScale: 1, exportWithDarkMode: false, exportEmbedScene: false, exportBackground: true };
  const render = (size: number) => exportToBlob({ elements, files, appState, mimeType: "image/png", maxWidthOrHeight: size });
  let blob = await render(1280);
  if (blob.size > 4.4 * 1024 * 1024) blob = await render(1024);
  if (blob.size > 4.4 * 1024 * 1024) throw new Error("Candidate image exceeds vision transfer limit");
  // The native exporter loads fonts; require successful web-font loading rather
  // than publishing its silent fallback. Helvetica (2) is a native local font.
  for (const family of new Set(texts.map(e => e.type === "text" ? e.fontFamily : 9))) {
    const name = Object.entries(FONT_FAMILY).find(([, value]) => value === family)?.[0];
    if (!name) throw new Error("Candidate font unavailable");
    const text = texts.filter(e => e.type === "text" && e.fontFamily === family).map(e => e.type === "text" ? e.text : "").join("");
    const font = `16px "${name}"`, faces = await document.fonts.load(font, text);
    if (!document.fonts.check(font, text) || (family !== 2 && !faces.length)) throw new Error("Candidate font unavailable");
  }
  const dataURL = await new Promise<string>((resolve, reject) => {
    const reader = new FileReader(); reader.onload = () => resolve(String(reader.result)); reader.onerror = () => reject(reader.error); reader.readAsDataURL(blob);
  });
  return { dataURL, renderer: fingerprint ? `excalidraw-browser/font9/${fingerprint}` : "excalidraw-browser" };
}
/** Kept-alive canvases service only explicitly requested candidate renders. */
export function useCandidatePreviewRenderer(projectId: string | undefined, figureId: string, ready: boolean) {
  const [status, setStatus] = useState<string | null>(null);
  useEffect(() => {
    if (!ready || !projectId) return;
    let cancelled = false, timer: ReturnType<typeof setTimeout> | undefined;
    const path = `/visualization/projects/${encodeURIComponent(projectId)}/figures/${encodeURIComponent(figureId)}`;
    const tick = async () => {
      let delay = 1500;
      try {
        const requests = await gatewayRequest("GET", `${path}/proposal-preview-requests`) as Request[];
        for (const request of requests) {
          if (cancelled) return;
          setStatus("正在生成候选图预览…");
          let completion: { dataURL: string; renderer: string } | { error: string; renderer: string };
          try { completion = await renderCandidatePreview(request.document); }
          catch { completion = { error: "候选图导出失败，请检查素材文件与字体；修复后可重新请求预览。", renderer: "excalidraw-browser" }; }
          if (cancelled) return;
          try {
            await gatewayRequest("POST", `${path}/proposals/${encodeURIComponent(request.proposal_id)}/preview`, { base_revision: request.base_revision, document_sha256: request.document_sha256, ...completion });
          } catch (cause) {
            if (!(cause instanceof GatewayHttpError && [404, 409].includes(cause.status))) throw cause;
          }
        }
        if (!cancelled) setStatus(null);
      } catch { if (!cancelled) setStatus("候选图预览服务暂不可用，稍后重试。"); delay = 5000; }
      finally { if (!cancelled) timer = setTimeout(() => void tick(), delay); }
    };
    void tick();
    return () => { cancelled = true; clearTimeout(timer); };
  }, [projectId, figureId, ready]);
  return status;
}
