import { createRoot } from "react-dom/client";
import { useState } from "react";
import { WikiEntrySurface } from "../src/features/wiki/wiki-editor";
import type { WikiMode } from "../src/features/wiki/reading-view";
import { Toaster } from "sonner";
let root: ReturnType<typeof createRoot> | undefined;
export function unmount() { root?.unmount(); }
export function mount(entryId: string, split = false) {
  function Surface({ pane }: { pane: string }) {
    const [mode, setMode] = useState<WikiMode>(split ? "edit" : "read");
    return <div data-wiki-pane={pane} style={{ flex: 1, minWidth: 0, display: "flex" }}><WikiEntrySurface entryId={entryId} mode={mode} onModeChange={setMode} onNavigate={() => {}} /></div>;
  }
  root = createRoot(document.body);
  root.render(<div style={{ height: "100vh", display: "flex" }}><Surface pane="first" />{split && <Surface pane="second" />}<Toaster /></div>);
}
