/**
 * Editor-kind registry: maps an editor's `kind` to its presentation and
 * mounting behaviour. Feature modules register kinds at module load (the
 * shell imports the registration modules for side effects only); the store
 * stays presentation-free.
 */

import type { ComponentType } from "react";
import type { LucideIcon } from "lucide-react";
import type { EditorRef } from "./workbench-store";

/** What an editor body receives. `api` is the workbench store module
 *  (imported directly by features); `runtime` arrives through
 *  WorkbenchRuntimeContext, so bodies can read agent state. */
export type EditorContext = { editor: EditorRef };

export type EditorKindCommand = { id: string; title: string; run: () => void };

export type EditorKind = {
  kind: string;
  /** Tab label. */
  title(editor: EditorRef): string;
  /** Optional longer name for quick-open. */
  name?(editor: EditorRef): string;
  icon?(editor: EditorRef): LucideIcon | undefined;
  body: ComponentType<EditorContext>;
  /** Explicit, bounded data safe to share with the agent at message time. */
  agentContext?(editor: EditorRef): Record<string, string | number | boolean | null> | null;
  /** keep-alive: body stays mounted (hidden) while its tab exists.
   *  ephemeral: body mounts only while the tab is the group's active tab;
   *  a placeholder shows otherwise (readers persist their own state). */
  mount: "keep-alive" | "ephemeral";
  /** How inactive keep-alive bodies hide. "display" (default) unmounts
   *  layout via the hidden attribute; "visibility" keeps layout alive so
   *  scroll positions / canvas state survive tab switches (readers). */
  hideMode?: "display" | "visibility";
  /** Return true to veto closing (e.g. unsaved wiki edits). */
  confirmLeave?(editor: EditorRef): boolean;
  /** Per-editor commands contributed to the palette while active. */
  commands?(editor: EditorRef): EditorKindCommand[];
};

const kinds = new Map<string, EditorKind>();

export function registerEditorKind(kind: EditorKind): void {
  kinds.set(kind.kind, kind);
}

export function editorKind(kind: string): EditorKind | undefined {
  return kinds.get(kind);
}

export function allEditorKinds(): EditorKind[] {
  return [...kinds.values()];
}
