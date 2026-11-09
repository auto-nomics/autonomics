import { useEffect, useState } from "react";
import { Button } from "@/components/ui/button";
import { openEditor } from "@/app/workbench/workbench-store";
import { createCanvas, refreshCanvases, useCanvases } from "./canvas-store";
export function CanvasSidebar() {
  const canvases = useCanvases();
  const [error, setError] = useState("");
  const [busy, setBusy] = useState(false);
  useEffect(() => { void refreshCanvases().catch(cause => setError(String(cause))); }, []);
  return <section aria-label="画布列表" className="flex min-h-0 flex-1 flex-col gap-2 overflow-auto p-2">
    <Button disabled={busy} onClick={async () => {
      setBusy(true); setError("");
      try { const canvas = await createCanvas(); openEditor({ kind: "canvas", params: { canvasId: canvas.id }, state: { title: canvas.title } }); }
      catch (cause) { setError(String(cause)); } finally { setBusy(false); }
    }}>新建画布</Button>
    {error && <p role="alert" className="text-xs text-destructive">{error}</p>}
    {canvases.map(canvas => <Button key={canvas.id} variant="ghost" className="justify-start" onClick={() => openEditor({ kind: "canvas", params: { canvasId: canvas.id }, state: { title: canvas.title } })}>{canvas.title}</Button>)}
    {!canvases.length && <p className="p-2 text-xs text-muted-foreground">创建画布，添加图形、连线或图片。</p>}
  </section>;
}
