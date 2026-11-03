/**
 * M1 editor kinds: the transitional whole-view editors. Bibliography and
 * wiki keep their composite view components intact (M2/M3 decompose them);
 * config and dag are the first native workbench editors.
 */

import { BookOpen, GitFork, Library, Settings } from "lucide-react";
import type { BibliographyWorkspace } from "@/features/bibliography";
import { BibliographyView } from "@/features/bibliography";
import { WikiPanel } from "@/features/wiki";
import { ConfigView, DagView } from "@/features/system";
import { leafName } from "@/features/agent";
import { registerEditorKind, type EditorContext } from "./editor-registry";
import { useWorkbenchRuntime } from "./runtime-context";

function BibliographyEditorBody({ editor }: EditorContext) {
  const workspace = (editor.params.workspace as BibliographyWorkspace | undefined) ?? "library";
  return (
    <section aria-label="文献工作区" className="flex min-h-0 min-w-0 flex-1 flex-col">
      <BibliographyView workspace={workspace} />
    </section>
  );
}

function WikiEditorBody() {
  return (
    <section aria-label="知识库工作区" className="flex min-h-0 min-w-0 flex-1 flex-col">
      <WikiPanel />
    </section>
  );
}

function ConfigEditorBody() {
  const runtime = useWorkbenchRuntime();
  return (
    <div className="min-h-0 flex-1 overflow-auto p-3">
      <ConfigView activeAgent={runtime.activeAgent} />
    </div>
  );
}

function DagEditorBody({ editor }: EditorContext) {
  const agentPath = String(editor.params.agentPath ?? "");
  return (
    <div className="flex min-h-0 flex-1 flex-col">
      <DagView key={agentPath} activeAgent={agentPath} />
    </div>
  );
}

let registered = false;

/** Idempotent registration; imported by the shell for side effects. */
export function registerWorkbenchEditorKinds(): void {
  if (registered) return;
  registered = true;
  registerEditorKind({
    kind: "bibliography",
    title: () => "文献与证据",
    name: () => "Bibliography",
    icon: () => Library,
    body: BibliographyEditorBody,
    mount: "keep-alive",
  });
  registerEditorKind({
    kind: "wiki",
    title: () => "知识库",
    name: () => "Knowledge base",
    icon: () => BookOpen,
    body: WikiEditorBody,
    mount: "keep-alive",
  });
  registerEditorKind({
    kind: "config",
    title: () => "设置",
    name: () => "Settings",
    icon: () => Settings,
    body: ConfigEditorBody,
    mount: "keep-alive",
  });
  registerEditorKind({
    kind: "dag",
    title: (editor) => `DAG · ${leafName(String(editor.params.agentPath ?? ""))}`,
    name: (editor) => `Agent DAG (${String(editor.params.agentPath ?? "")})`,
    icon: () => GitFork,
    body: DagEditorBody,
    mount: "keep-alive",
  });
}
