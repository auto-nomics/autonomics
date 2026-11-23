import { useState } from "react";
import { useTranslation } from "react-i18next";
import { CaptureUpdateAction, convertToExcalidrawElements } from "@excalidraw/excalidraw";
import type { BinaryFileData, ExcalidrawImperativeAPI } from "@excalidraw/excalidraw/types";
import { gatewayRequest } from "@/gateway";
import { imagePlacement } from "./canvas-image-placement";
import { Button } from "@/components/ui/button";
type Asset = { id: string; name: string; category: string; license: string; license_url: string | null; author: string; source_url: string };
type VectorAsset = { asset: Asset; dataURL: string; previewDataURL: string; width: number; height: number; sha256: string; source_sha256: string; colors: string[]; normalization: string[] };
export function useBioIconsState() {
  const [query, setQuery] = useState(""), [assets, setAssets] = useState<Asset[]>([]), [preview, setPreview] = useState<VectorAsset | null>(null);
  const [author, setAuthor] = useState("");
  const [searchedQuery, setSearchedQuery] = useState<string | null>(null);
  const [appliedAuthor, setAppliedAuthor] = useState<string | null>(null);
  const [partialOnly, setPartialOnly] = useState(false);
  const [busy, setBusy] = useState(false), [error, setError] = useState("");
  return { query, setQuery, assets, setAssets, preview, setPreview, author, setAuthor, searchedQuery, setSearchedQuery, appliedAuthor, setAppliedAuthor, partialOnly, setPartialOnly, busy, setBusy, error, setError };
}
export function BioIconsPanel({ getApi, embedded = false, controller }: { getApi: () => ExcalidrawImperativeAPI | null; embedded?: boolean; controller?: ReturnType<typeof useBioIconsState> }) {
  const { t } = useTranslation("canvas");
  const local = useBioIconsState();
  const { query, setQuery, assets, setAssets, preview, setPreview, author, setAuthor, searchedQuery, setSearchedQuery, appliedAuthor, setAppliedAuthor, partialOnly, setPartialOnly, busy, setBusy, error, setError } = controller ?? local;
  async function load(asset: Asset) { return await gatewayRequest("GET", `/visualization/bioicons/${encodeURIComponent(asset.id)}/vector`) as VectorAsset; }
  const Container = embedded ? "section" : "aside";
  return <Container aria-label={t("bioicons.panelLabel")} className={embedded ? "canvas-catalog-panel min-w-0 overflow-auto p-3 text-sm" : "w-72 shrink-0 overflow-auto border-l p-2 text-sm"}><form className="flex flex-col gap-2" onSubmit={async event => {
    event.preventDefault(); if (busy || !query.trim()) return; setBusy(true); setError("");
    try { const result = await gatewayRequest("GET", `/visualization/bioicons/search?q=${encodeURIComponent(query.trim())}${author.trim() ? `&author=${encodeURIComponent(author.trim())}` : ""}`) as { assets: Asset[]; author_filter?: string; match_summary?: { exact_name: number; name_tokens: number } }; setAssets(result.assets); setSearchedQuery(query.trim()); setAppliedAuthor(result.author_filter ?? null); setPreview(null); setPartialOnly(Boolean(result.assets.length && result.match_summary && result.match_summary.exact_name + result.match_summary.name_tokens === 0)); }
    catch (cause) { setError(String(cause)); } finally { setBusy(false); }
  }}><div className="flex gap-1"><input aria-label={t("bioicons.searchLabel")} placeholder={t("bioicons.searchPlaceholder")} className="min-w-0 flex-1 rounded border bg-background px-2" value={query} onChange={event => setQuery(event.target.value)} /><Button type="submit" size="sm" disabled={busy || !query.trim()}>{t("bioicons.search")}</Button></div><label className="text-xs">{t("bioicons.authorFilter")}<input aria-label={t("bioicons.authorLabel")} placeholder={t("bioicons.authorPlaceholder")} maxLength={300} className="mt-1 w-full rounded border bg-background px-2 py-1" value={author} onChange={event=>setAuthor(event.target.value)} /></label></form>
    <p className="my-2 text-xs text-muted-foreground">{t("bioicons.note")}</p>
    {appliedAuthor && <p role="status" className="my-2 text-xs text-muted-foreground">{t("bioicons.appliedAuthor", { author: appliedAuthor })}</p>}
    {searchedQuery && assets.length === 0 && <p className="my-2 text-xs text-muted-foreground">{appliedAuthor ? t("bioicons.noMatchAuthor", { query: searchedQuery }) : t("bioicons.noMatch", { query: searchedQuery })}</p>}
    {partialOnly && <p role="status" className="my-2 text-xs text-muted-foreground">{t("bioicons.partialOnly")}</p>}
    {error && <p role="alert" className="my-2 text-destructive">{error}</p>}
    {assets.map(asset => <article key={asset.id} className="my-2 rounded border p-2"><p>{asset.name.replaceAll('_', ' ')}</p><p className="text-xs text-muted-foreground">{asset.author} · {asset.license}</p>{preview?.asset.id === asset.id && <img alt={asset.name} src={preview.previewDataURL} className="my-2 max-h-48 w-full rounded bg-white object-contain" />}<div className="mt-2 flex flex-wrap gap-2"><a href={asset.source_url} target="_blank" rel="noreferrer" className="underline">{t("bioicons.source")}</a>{asset.license_url && <a href={asset.license_url} target="_blank" rel="noreferrer" className="underline">{t("bioicons.license")}</a>}<Button size="sm" variant="outline" disabled={busy} onClick={async () => { setBusy(true); setError(""); try { setPreview(await load(asset)); } catch (cause) { setError(String(cause)); } finally { setBusy(false); } }}>{t("bioicons.preview")}</Button><Button size="sm" disabled={busy} onClick={async () => {
      setBusy(true); setError("");
      try {
        const vector = preview?.asset.id === asset.id ? preview : await load(asset); const image = new Image(); image.src = vector.dataURL; await image.decode();
        const api = getApi(); if (!api) throw new Error(t("bioicons.canvasNotReady")); const scale = Math.min(1, 240 / Math.max(vector.width, vector.height));
        const fileId = vector.sha256 as BinaryFileData["id"];
        const placement = imagePlacement(api, vector.width * scale, vector.height * scale);
        const elements = convertToExcalidrawElements([{ type: "image", x: placement.x, y: placement.y, width: vector.width * scale, height: vector.height * scale, fileId, status: "saved", customData: { source: "BioIcons", asset: vector.asset, sha256: vector.sha256, source_sha256: vector.source_sha256, colors: vector.colors, normalization: vector.normalization } }]);
        api.addFiles([{ id: fileId, dataURL: vector.dataURL as BinaryFileData["dataURL"], mimeType: "image/svg+xml", created: Date.now() }]);
        api.updateScene({ elements: [...api.getSceneElements(), ...elements], appState: { selectedElementIds: { [elements[0].id]: true } }, captureUpdate: CaptureUpdateAction.IMMEDIATELY });
        if (placement.pan) api.scrollToContent(elements, { animate: false });
      } catch (cause) { setError(String(cause)); } finally { setBusy(false); }
    }}>{t("bioicons.insert")}</Button></div></article>)}
  </Container>;
}
