/**
 * Workbench command palette (Ctrl+P all / Ctrl+Shift+P commands-only).
 * UI relocated from the agent view palette; the command list now merges
 * static shell commands, feature-registered commands and the active
 * editor's kind commands, plus a quick "open editor tab" section.
 */

import { useMemo, useState } from "react";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Dialog, DialogContent, DialogDescription, DialogHeader, DialogTitle } from "@/components/ui/dialog";
import { Input } from "@/components/ui/input";
import { ScrollArea } from "@/components/ui/scroll-area";
import { editorKind } from "./editor-registry";
import {
  closePalette,
  useAllCommands,
  usePaletteState,
  type WorkbenchCommand,
} from "./command-registry";
import {
  activateEditor,
  useWorkbenchState,
  type EditorRef,
  type GroupId,
} from "./workbench-store";

type Row = WorkbenchCommand & { section: "editors" | "commands" };

export function CommandPalette() {
  const palette = usePaletteState();
  const commands = useAllCommands();
  const state = useWorkbenchState();
  const [query, setQuery] = useState("");

  const editorRows = useMemo<Row[]>(() => {
    if (palette.mode === "commands") return [];
    const rows: Row[] = [];
    const groups: Array<[GroupId, typeof state.groups.main]> = [
      ["main", state.groups.main],
    ];
    if (state.groups.secondary) groups.push(["secondary", state.groups.secondary]);
    for (const [groupId, group] of groups) {
      for (const editor of group.editors) {
        const kind = editorKind(editor.kind);
        if (!kind) continue;
        rows.push({
          id: `editor:${groupId}:${editor.id}`,
          title: kind.title(editor),
          category: groupId === "main" ? "editor" : "editor (split)",
          keywords: `tab open switch focus ${editor.kind}`,
          section: "editors",
          run: () => activateEditor(groupId, editor.id),
        });
      }
    }
    return rows;
  }, [palette.mode, state.groups]);

  const commandRows = useMemo<Row[]>(() => {
    const rows = commands.map((command) => ({ ...command, section: "commands" as const }));
    // Per-active-editor kind commands ride at the end, namespaced.
    const group = state.groups.active === "secondary" ? state.groups.secondary : state.groups.main;
    const active: EditorRef | undefined = group?.editors.find((editor) => editor.id === group?.activeId);
    if (active) {
      const kind = editorKind(active.kind);
      for (const command of kind?.commands?.(active) ?? []) {
        rows.push({ id: `kind:${active.id}:${command.id}`, title: command.title, category: "editor", run: command.run, section: "commands" });
      }
    }
    return rows;
  }, [commands, state.groups]);

  const rows = useMemo(() => {
    const needle = query.trim().toLowerCase();
    const all = [...editorRows, ...commandRows];
    if (!needle) return all;
    return all.filter((row) => `${row.title} ${row.category ?? ""} ${row.keywords ?? ""}`.toLowerCase().includes(needle));
  }, [editorRows, commandRows, query]);

  if (!palette.open) return null;

  return (
    <Dialog open onOpenChange={(open) => { if (!open) closePalette(); }}>
      <DialogContent className="sm:max-w-xl">
        <DialogHeader>
          <DialogTitle>Command palette</DialogTitle>
          <DialogDescription>Ctrl+P opens · type to filter · Enter runs.</DialogDescription>
        </DialogHeader>
        <Input
          value={query}
          placeholder="Search commands"
          autoFocus
          onKeyDown={(event) => {
            if (event.key === "Enter") {
              event.preventDefault();
              rows[0]?.run();
              closePalette();
            }
          }}
          onChange={(event) => setQuery(event.target.value)}
        />
        <ScrollArea className="max-h-[60vh] min-h-0" contentClassName="space-y-1">
          {rows.map((row) => (
            <button
              key={row.id}
              type="button"
              onClick={() => {
                row.run();
                closePalette();
              }}
              className="grid w-full grid-cols-[minmax(0,1fr)_auto] items-center gap-2 rounded-md border p-2 text-left hover:bg-accent"
            >
              <span className="truncate text-sm">{row.title}</span>
              {row.category && <Badge variant="outline">{row.category}</Badge>}
            </button>
          ))}
          {!rows.length && <div className="p-4 text-sm text-muted-foreground">No matching commands</div>}
        </ScrollArea>
      </DialogContent>
    </Dialog>
  );
}

/** Shared footer hint button style for the title-bar command center. */
export function CommandCenterButton({ onClick }: { onClick: () => void }) {
  return (
    <Button
      variant="outline"
      size="sm"
      className="h-7 w-64 justify-start gap-2 px-2.5 text-xs text-muted-foreground max-sm:w-full"
      onClick={onClick}
      aria-label="命令面板"
      title="命令面板(Ctrl+P)"
    >
      <span className="truncate">搜索或输入命令…</span>
      <kbd className="ml-auto shrink-0 rounded border bg-muted px-1 font-mono text-[10px] text-muted-foreground">Ctrl P</kbd>
    </Button>
  );
}
