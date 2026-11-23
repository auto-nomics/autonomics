import { useCallback, useEffect, useState } from "react";
import i18n from "@/i18n";
import { useWorkbenchRuntime } from "@/app/workbench/runtime-context";
import { findEditor, openAgentEditor, patchEditorState } from "@/app/workbench/workbench-store";

/** Stable per-figure specialist identity survives tab closure and application restarts. */
export async function canvasIllustratorName(projectId: string, canvasId: string) {
  const bytes = new TextEncoder().encode(JSON.stringify([projectId, canvasId]));
  const hash = await crypto.subtle.digest("SHA-256", bytes);
  return `illustrator_${Array.from(new Uint8Array(hash)).slice(0, 10).map(byte => byte.toString(16).padStart(2, "0")).join("")}`;
}
export function useCanvasIllustrator(editorId: string, projectId: string | undefined, canvasId: string, loaded: boolean) {
  const runtime = useWorkbenchRuntime();
  const [path, setPath] = useState("");
  const [error, setError] = useState("");
  const [attempt, setAttempt] = useState(0);
  useEffect(() => {
    if (!projectId || !loaded) return;
    let cancelled = false;
    setError("");
    void canvasIllustratorName(projectId, canvasId).then(runtime.ensureIllustratorAgent).then(path => {
      if (cancelled) return;
      setPath(path);
      const editor = findEditor(editorId);
      if (editor) patchEditorState(editorId, { ...(editor.state && typeof editor.state === "object" ? editor.state : {}), illustratorPath: path });
      openAgentEditor(path, false);
    }).catch(cause => { if (!cancelled) setError(i18n.t("canvas:illustrator.connectFailed", { cause: String(cause) })); });
    return () => { cancelled = true; };
  }, [editorId, projectId, canvasId, loaded, attempt, runtime.ensureIllustratorAgent]);
  const openConversation = useCallback(() => {
    if (path) { runtime.selectAgent(path); openAgentEditor(path); }
    else setAttempt(value => value + 1);
  }, [path, runtime.selectAgent]);
  return { path, error, openConversation };
}
