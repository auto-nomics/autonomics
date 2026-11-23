import { useState } from "react";
import { useTranslation } from "react-i18next";
import { convertToExcalidrawElements, CaptureUpdateAction } from "@excalidraw/excalidraw";
import type { BinaryFileData, ExcalidrawImperativeAPI } from "@excalidraw/excalidraw/types";
import { gatewayRequest } from "@/gateway";
import { imagePlacement } from "./canvas-image-placement";
import { Button } from "@/components/ui/button";
type Asset = { id: number; title: string; source_url: string; licenses: string[]; creators: string[] };
type Format = "png" | "svg";
type AssetImage = { asset: Asset; sha256: string; dataURL: string; file: unknown; format?: Format; width?: number; height?: number; previewDataURL?: string; provenance?: Record<string, unknown> };
export function useBioArtState() {
  const [query, setQuery] = useState("");
  const [format, setFormat] = useState<Format>("png");
  const [assets, setAssets] = useState<Asset[]>([]);
  const [preview, setPreview] = useState<AssetImage | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  return { query, setQuery, format, setFormat, assets, setAssets, preview, setPreview, busy, setBusy, error, setError };
}
export function BioArtPanel({ getApi, embedded = false, controller }: { getApi: () => ExcalidrawImperativeAPI | null; embedded?: boolean; controller?: ReturnType<typeof useBioArtState> }) {
  const { t } = useTranslation("canvas");
  const local = useBioArtState();
  const { query, setQuery, format, setFormat, assets, setAssets, preview, setPreview, busy, setBusy, error, setError } = controller ?? local;
  async function previewAsset(id: number) {
    setBusy(true); setError("");
    try { setPreview(await gatewayRequest("GET", `/visualization/bioart/${id}/image?format=${format}`) as AssetImage); }
    catch (cause) { setError(String(cause)); } finally { setBusy(false); }
  }
  async function insert(id: number) {
    setBusy(true); setError("");
    try {
      const result = preview?.asset.id === id && (preview.format ?? "png") === format ? preview : await gatewayRequest("GET", `/visualization/bioart/${id}/image?format=${format}`) as AssetImage;
      const image = new Image(); image.src = result.dataURL; await image.decode();
      const api = getApi(); if (!api) return;
      const fileId = result.sha256 as BinaryFileData["id"];
      const width = result.width ?? image.naturalWidth, height = result.height ?? image.naturalHeight;
      const scale = Math.min(1, 240 / Math.max(width, height));
      const placement = imagePlacement(api, width * scale, height * scale);
      const { x, y } = placement;
      const elements = convertToExcalidrawElements([{ type: "image", x, y, width: width * scale, height: height * scale, fileId, status: "saved", customData: result.provenance ?? { source: "NIH BioArt", asset: result.asset, sha256: result.sha256, file: result.file } }]);
      api.addFiles([{ id: fileId, dataURL: result.dataURL as BinaryFileData["dataURL"], mimeType: format === "svg" ? "image/svg+xml" : "image/png", created: Date.now() }]);
      api.updateScene({ elements: [...api.getSceneElements(), ...elements], appState: { selectedElementIds: { [elements[0].id]: true } }, captureUpdate: CaptureUpdateAction.IMMEDIATELY });
      if (placement.pan) api.scrollToContent(elements, { animate: false });
    } catch (cause) { setError(String(cause)); } finally { setBusy(false); }
  }
  const Container = embedded ? "section" : "aside";
  return <Container aria-label={t("bioart.panelLabel")} className={embedded ? "canvas-catalog-panel min-w-0 overflow-auto p-3 text-sm" : "w-72 shrink-0 overflow-auto border-l p-2 text-sm"}>
    <form className="flex gap-1" onSubmit={async event => {
      event.preventDefault(); if (busy || !query.trim()) return; setBusy(true); setError("");
      try { const result = await gatewayRequest("GET", `/visualization/bioart/search?q=${encodeURIComponent(query.trim())}`) as { assets: Asset[] }; setAssets(result.assets); setPreview(null); }
      catch (cause) { setError(String(cause)); } finally { setBusy(false); }
    }}><input aria-label={t("bioart.searchLabel")} placeholder={t("bioart.searchPlaceholder")} className="min-w-0 flex-1 rounded border bg-background px-2" value={query} onChange={event => setQuery(event.target.value)} /><Button type="submit" size="sm" disabled={busy || !query.trim()}>{t("bioart.search")}</Button></form>
    <label className="my-2 block text-xs">{t("bioart.insertFormat")}<select aria-label={t("bioart.formatLabel")} className="ml-2 rounded border bg-background p-1" disabled={busy} value={format} onChange={event => { setFormat(event.target.value as Format); setPreview(null); setError(""); }}><option value="png">{t("bioart.formatPng")}</option><option value="svg">{t("bioart.formatSvg")}</option></select></label>
    {error && <p role="alert" className="my-2 text-destructive">{error}</p>}
    <p className="my-2 text-xs text-muted-foreground">{t("bioart.note")}</p>
    {assets.map(asset => <article key={asset.id} className="my-2 rounded border p-2"><p>{asset.title}</p>{preview?.asset.id === asset.id && <figure className="my-2"><img alt={preview.asset.title} src={preview.previewDataURL ?? preview.dataURL} className="max-h-48 w-full rounded bg-white object-contain" /><figcaption className="mt-1 text-xs text-muted-foreground">{preview.asset.creators?.join(" · ") || t("bioart.noAuthors")}</figcaption></figure>}<p className="text-xs text-muted-foreground">{asset.licenses.join(" · ") || t("bioart.noLicense")}</p><div className="mt-2 flex flex-wrap items-center gap-2"><a href={asset.source_url} target="_blank" rel="noreferrer" className="underline">{t("bioart.source")}</a><Button size="sm" variant="outline" disabled={busy} onClick={() => void previewAsset(asset.id)}>{t("bioart.previewImage")}</Button><Button size="sm" disabled={busy} onClick={() => void insert(asset.id)}>{t("bioart.insertImage")}</Button></div></article>)}
  </Container>;
}
