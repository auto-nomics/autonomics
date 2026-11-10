import { useState } from "react";
import { CaptureUpdateAction, convertToExcalidrawElements } from "@excalidraw/excalidraw";
import type { BinaryFileData, ExcalidrawImperativeAPI } from "@excalidraw/excalidraw/types";
import { gatewayRequest } from "@/gateway";
import { imagePlacement } from "./canvas-image-placement";
import { Button } from "@/components/ui/button";
type Asset = { id: string; name: string; category: string; license: string; license_url: string | null; author: string; source_url: string };
type VectorAsset = { asset: Asset; dataURL: string; previewDataURL: string; width: number; height: number; sha256: string; source_sha256: string; colors: string[]; normalization: string[] };
export function BioIconsPanel({ getApi }: { getApi: () => ExcalidrawImperativeAPI | null }) {
  const [query, setQuery] = useState(""), [assets, setAssets] = useState<Asset[]>([]), [preview, setPreview] = useState<VectorAsset | null>(null);
  const [busy, setBusy] = useState(false), [error, setError] = useState("");
  async function load(asset: Asset) { return await gatewayRequest("GET", `/visualization/bioicons/${encodeURIComponent(asset.id)}/vector`) as VectorAsset; }
  return <aside aria-label="BioIcons 矢量素材" className="w-72 shrink-0 overflow-auto border-l p-2 text-sm"><form className="flex gap-1" onSubmit={async event => {
    event.preventDefault(); if (busy || !query.trim()) return; setBusy(true); setError("");
    try { const result = await gatewayRequest("GET", `/visualization/bioicons/search?q=${encodeURIComponent(query.trim())}`) as { assets: Asset[] }; setAssets(result.assets); setPreview(null); }
    catch (cause) { setError(String(cause)); } finally { setBusy(false); }
  }}><input aria-label="BioIcons 搜索词" placeholder="如 receptor、cell" className="min-w-0 flex-1 rounded border bg-background px-2" value={query} onChange={event => setQuery(event.target.value)} /><Button type="submit" size="sm" disabled={busy || !query.trim()}>搜索</Button></form>
    <p className="my-2 text-xs text-muted-foreground">SVG 作为独立矢量图片插入，保留作者、各素材许可与来源。内部路径暂不拆分为画布对象。</p>
    {error && <p role="alert" className="my-2 text-destructive">{error}</p>}
    {assets.map(asset => <article key={asset.id} className="my-2 rounded border p-2"><p>{asset.name.replaceAll('_', ' ')}</p><p className="text-xs text-muted-foreground">{asset.author} · {asset.license}</p>{preview?.asset.id === asset.id && <img alt={asset.name} src={preview.previewDataURL} className="my-2 max-h-48 w-full rounded bg-white object-contain" />}<div className="mt-2 flex flex-wrap gap-2"><a href={asset.source_url} target="_blank" rel="noreferrer" className="underline">来源</a>{asset.license_url && <a href={asset.license_url} target="_blank" rel="noreferrer" className="underline">许可</a>}<Button size="sm" variant="outline" disabled={busy} onClick={async () => { setBusy(true); setError(""); try { setPreview(await load(asset)); } catch (cause) { setError(String(cause)); } finally { setBusy(false); } }}>预览矢量素材</Button><Button size="sm" disabled={busy} onClick={async () => {
      setBusy(true); setError("");
      try {
        const vector = preview?.asset.id === asset.id ? preview : await load(asset); const image = new Image(); image.src = vector.dataURL; await image.decode();
        const api = getApi(); if (!api) throw new Error("画布未就绪"); const scale = Math.min(1, 240 / Math.max(vector.width, vector.height));
        const fileId = vector.sha256 as BinaryFileData["id"];
        const placement = imagePlacement(api, vector.width * scale, vector.height * scale);
        const elements = convertToExcalidrawElements([{ type: "image", x: placement.x, y: placement.y, width: vector.width * scale, height: vector.height * scale, fileId, status: "saved", customData: { source: "BioIcons", asset: vector.asset, sha256: vector.sha256, source_sha256: vector.source_sha256, colors: vector.colors, normalization: vector.normalization } }]);
        api.addFiles([{ id: fileId, dataURL: vector.dataURL as BinaryFileData["dataURL"], mimeType: "image/svg+xml", created: Date.now() }]);
        api.updateScene({ elements: [...api.getSceneElements(), ...elements], appState: { selectedElementIds: { [elements[0].id]: true } }, captureUpdate: CaptureUpdateAction.IMMEDIATELY });
        if (placement.pan) api.scrollToContent(elements, { animate: false });
      } catch (cause) { setError(String(cause)); } finally { setBusy(false); }
    }}>插入矢量素材</Button></div></article>)}
  </aside>;
}
