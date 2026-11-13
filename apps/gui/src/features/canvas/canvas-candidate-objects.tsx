import { useMemo, useState } from "react";
import type { ExcalidrawInitialDataState } from "@excalidraw/excalidraw/types";
export function CanvasCandidateObjects({ document, focusedId, onFocus }: {
  document: ExcalidrawInitialDataState; focusedId?: string; onFocus: (id: string | null) => void;
}) {
  const [query, setQuery] = useState("");
  const objects = useMemo(() => (document.elements ?? []).filter(e => !e.isDeleted).map(e => {
    const text = "text" in e && typeof e.text === "string" ? e.text : "";
    const scientificId = typeof e.customData?.scientificId === "string" ? e.customData.scientificId : "";
    return { id: e.id, label: `${e.type} · ${text || scientificId || e.id}`, search: `${e.id} ${e.type} ${text} ${scientificId}`.toLocaleLowerCase() };
  }), [document]);
  const matched = objects.filter(e => e.search.includes(query.trim().toLocaleLowerCase()));
  const shown = matched.slice(0, 200);
  const current = focusedId ? objects.find(e => e.id === focusedId) : undefined;
  return <section aria-label="候选对象定位" className="mt-3 space-y-2 border-t pt-3 text-xs">
    <p className="font-medium">核对候选对象</p>
    <input aria-label="搜索候选对象" className="w-full rounded border bg-background p-1" placeholder="按文字、类型或 ID 搜索" value={query} onChange={event => setQuery(event.target.value)} />
    <select aria-label="候选对象" className="w-full rounded border bg-background p-1" value={focusedId ?? ""} onChange={event => onFocus(event.target.value || null)}>
      <option value="">整份候选</option>
      {focusedId && !shown.some(e => e.id === focusedId) && <option value={focusedId}>{current ? current.label.slice(0, 100) : `对照版本对象 · ${focusedId}`}</option>}
      {shown.map(e => <option key={e.id} value={e.id}>{e.label.slice(0, 100)}</option>)}
    </select>
    <p role="status">匹配 {matched.length} / {objects.length} 个候选对象{matched.length > shown.length ? "；仅显示前 200 项，请缩小搜索范围。" : ""}</p>
    <p>包含尚未修改的对象；选择后可在下方提出本轮修改要求。</p>
  </section>;
}
