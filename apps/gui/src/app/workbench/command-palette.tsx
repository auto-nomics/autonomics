/**
 * Workbench command palette (Ctrl+P all / Ctrl+Shift+P commands-only).
 * Obsidian-style chrome: top-anchored minimal dialog — borderless query row
 * with a divider and the dialog close button, then flat cmdk rows reading
 * "Category: title" with shortcut chips on the right. cmdk owns filtering
 * (fuzzy over title + category + keywords), arrow navigation, wrap-around
 * and Enter; open-editor tabs ride above the commands as a quick switcher.
 */

import { useEffect, useMemo, useRef } from "react";
import { useTranslation } from "react-i18next";
import { Button } from "@/components/ui/button";
import {
  Command,
  CommandEmpty,
  CommandGroup,
  CommandInput,
  CommandItem,
  CommandList,
  CommandShortcut,
} from "@/components/ui/command";
import { Dialog, DialogContent, DialogDescription, DialogHeader, DialogTitle } from "@/components/ui/dialog";
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
  allGroups,
  getGroup,
} from "./workbench-store";

type Row = WorkbenchCommand & { section: "editors" | "commands" };

export function CommandPalette() {
  const { t } = useTranslation("workbench");
  const palette = usePaletteState();
  const commands = useAllCommands();
  const state = useWorkbenchState();
  const inputRef = useRef<HTMLInputElement>(null);

  // The palette mounts fresh on every open, so the query resets; refocus
  // explicitly — the dialog's initial-focus handling can beat autoFocus.
  useEffect(() => {
    if (palette.open) inputRef.current?.focus();
  }, [palette.open]);

  const editorRows = useMemo<Row[]>(() => {
    if (palette.mode === "commands") return [];
    const rows: Row[] = [];
    const groups = allGroups(state.groups);
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
    const group = getGroup(state.groups, state.groups.active);
    const active: EditorRef | undefined = group?.editors.find((editor) => editor.id === group?.activeId);
    if (active) {
      const kind = editorKind(active.kind);
      for (const command of kind?.commands?.(active) ?? []) {
        rows.push({ id: `kind:${active.id}:${command.id}`, title: command.title, category: "editor", run: command.run, section: "commands" });
      }
    }
    return rows;
  }, [commands, state.groups]);

  if (!palette.open) return null;

  return (
    <Dialog open onOpenChange={(open) => { if (!open) closePalette(); }}>
      <DialogContent className="top-[12%] translate-y-0 gap-0 overflow-hidden rounded-xl! p-0 sm:max-w-2xl">
        <DialogHeader className="sr-only">
          <DialogTitle>{t("palette.title")}</DialogTitle>
          <DialogDescription>{t("palette.description")}</DialogDescription>
        </DialogHeader>
        <Command loop className="gap-0 p-0">
          <CommandInput
            ref={inputRef}
            variant="plain"
            placeholder={palette.mode === "commands" ? t("palette.placeholderCommands") : t("palette.placeholderAll")}
            autoFocus
            className="text-[13px]"
          />
          <CommandList className="max-h-[min(26rem,60vh)] p-1">
            {editorRows.length > 0 && (
              <CommandGroup>
                {editorRows.map((row) => (
                  <PaletteRow key={row.id} row={row} />
                ))}
              </CommandGroup>
            )}
            <CommandGroup>
              {commandRows.map((row) => (
                <PaletteRow key={row.id} row={row} />
              ))}
            </CommandGroup>
            <CommandEmpty className="py-8 text-center text-sm text-muted-foreground">{t("palette.empty")}</CommandEmpty>
          </CommandList>
        </Command>
      </DialogContent>
    </Dialog>
  );
}

/** One palette row: inline "Category: title" label (commands only; editor
 *  tabs stay plain like a quick switcher) plus the optional shortcut chip. */
function PaletteRow({ row }: { row: Row }) {
  const category = row.category && row.section === "commands"
    ? row.category.charAt(0).toUpperCase() + row.category.slice(1)
    : null;
  return (
    <CommandItem
      // value must be unique for cmdk selection; searchable text rides in keywords.
      value={row.id}
      keywords={[`${row.title} ${row.category ?? ""} ${row.keywords ?? ""}`]}
      onSelect={() => {
        closePalette();
        row.run();
      }}
      className="data-selected:bg-accent py-2 text-[13px]"
    >
      <span className="min-w-0 flex-1 truncate">
        {category && (
          <span className="text-muted-foreground group-data-selected/command-item:text-foreground/70">{category}: </span>
        )}
        {row.title}
      </span>
      {row.shortcut && (
        <CommandShortcut className="rounded-md border bg-muted px-1.5 py-0.5 font-mono text-[10px] tracking-normal">
          {row.shortcut}
        </CommandShortcut>
      )}
    </CommandItem>
  );
}

/** Shared footer hint button style for the title-bar command center. */
export function CommandCenterButton({ onClick }: { onClick: () => void }) {
  const { t } = useTranslation("workbench");
  return (
    <Button
      variant="outline"
      size="sm"
      className="h-7 w-64 justify-start gap-2 px-2.5 text-xs text-muted-foreground max-sm:w-full"
      onClick={onClick}
      aria-label={t("commandCenter.aria")}
      title={t("commandCenter.title")}
    >
      <span className="truncate">{t("commandCenter.placeholder")}</span>
      <kbd className="ml-auto shrink-0 rounded border bg-muted px-1 font-mono text-[10px] text-muted-foreground">Ctrl P</kbd>
    </Button>
  );
}
