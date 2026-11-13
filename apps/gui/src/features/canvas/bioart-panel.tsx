import { useState } from "react";
import { convertToExcalidrawElements, CaptureUpdateAction } from "@excalidraw/excalidraw";
import type { BinaryFileData, ExcalidrawImperativeAPI } from "@excalidraw/excalidraw/types";
import { gatewayRequest } from "@/gateway";
import { imagePlacement } from "./canvas-image-placement";
import { Button } from "@/components/ui/button";
type Asset = { id: number; title: string; source_url: string; licenses: string[]; creators: string[] };
type Format = "png" | "svg";
type AssetImage = { asset: Asset; sha256: string; dataURL: string; file: unknown; format?: Format; width?: number; height?: number; previewDataURL?: string; provenance?: Record<string, unknown> };
export function BioArtPanel({ getApi }: { getApi: () => ExcalidrawImperativeAPI | null }) {
  const [query, setQuery] = useState("");
  const [format, setFormat] = useState<Format>("png");
  const [assets, setAssets] = useState<Asset[]>([]);
  const [preview, setPreview] = useState<AssetImage | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
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
  return <aside aria-label="BioArt 素材" className="w-64 shrink-0 overflow-auto border-l p-2 text-sm">
    <form className="flex gap-1" onSubmit={async event => {
      event.preventDefault(); if (busy || !query.trim()) return; setBusy(true); setError("");
      try { const result = await gatewayRequest("GET", `/visualization/bioart/search?q=${encodeURIComponent(query.trim())}`) as { assets: Asset[] }; setAssets(result.assets); setPreview(null); }
      catch (cause) { setError(String(cause)); } finally { setBusy(false); }
    }}><input aria-label="BioArt 搜索词" placeholder="搜索 BioArt，如 cell" className="min-w-0 flex-1 rounded border bg-background px-2" value={query} onChange={event => setQuery(event.target.value)} /><Button type="submit" size="sm" disabled={busy || !query.trim()}>搜索</Button></form>
    <label className="my-2 block text-xs">插入格式<select aria-label="BioArt 插入格式" className="ml-2 rounded border bg-background p-1" disabled={busy} value={format} onChange={event => { setFormat(event.target.value as Format); setPreview(null); setError(""); }}><option value="png">PNG 图片</option><option value="svg">SVG 矢量</option></select></label>
    {error && <p role="alert" className="my-2 text-destructive">{error}</p>}
    <p className="my-2 text-xs text-muted-foreground">素材独立插入，保留来源与许可。SVG 插入后可选中图片，通过“素材编辑”调整颜色与路径；不支持的格式会提示原因。</p>
    {assets.map(asset => <article key={asset.id} className="my-2 rounded border p-2"><p>{asset.title}</p>{preview?.asset.id === asset.id && <figure className="my-2"><img alt={preview.asset.title} src={preview.previewDataURL ?? preview.dataURL} className="max-h-48 w-full rounded bg-white object-contain" /><figcaption className="mt-1 text-xs text-muted-foreground">{preview.asset.creators?.join(" · ") || "作者信息未提供"}</figcaption></figure>}<p className="text-xs text-muted-foreground">{asset.licenses.join(" · ") || "许可信息未提供"}</p><div className="mt-2 flex flex-wrap items-center gap-2"><a href={asset.source_url} target="_blank" rel="noreferrer" className="underline">来源</a><Button size="sm" variant="outline" disabled={busy} onClick={() => void previewAsset(asset.id)}>预览图片</Button><Button size="sm" disabled={busy} onClick={() => void insert(asset.id)}>插入图片</Button></div></article>)}
  </aside>;
}
