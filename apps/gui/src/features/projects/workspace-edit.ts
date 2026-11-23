import * as monaco from "monaco-editor";
import i18n from "@/i18n";

type Position = { line: number; character: number };
type TextEdit = { range: { start: Position; end: Position }; newText: string };
export type FileEdits = { path: string; version: number | null; edits: TextEdit[] };

/** Only text edits within /work can be applied to project buffers. */
export function parseWorkspaceEdit(value: any): FileEdits[] {
  if (!value || typeof value !== "object" || (value.changes && value.documentChanges)) throw new Error(i18n.t("projects:workspaceEdit.invalid"));
  const entries: { uri: string; version: number | null; edits: unknown }[] = [];
  if (value.changes && typeof value.changes === "object" && !Array.isArray(value.changes)) {
    for (const [uri, edits] of Object.entries(value.changes)) entries.push({ uri, version: null, edits });
  } else if (Array.isArray(value.documentChanges)) {
    for (const change of value.documentChanges) {
      if (!change?.textDocument || typeof change.textDocument.uri !== "string" || !Array.isArray(change.edits)) throw new Error(i18n.t("projects:workspaceEdit.unsupportedChange"));
      entries.push({ uri: change.textDocument.uri, version: change.textDocument.version ?? null, edits: change.edits });
    }
  } else throw new Error(i18n.t("projects:workspaceEdit.noTextEdits"));
  if (entries.length > 100) throw new Error(i18n.t("projects:workspaceEdit.tooManyFiles"));
  const seen = new Set<string>();
  return entries.map(({ uri, version, edits }) => {
    const parsed = monaco.Uri.parse(uri);
    if (parsed.scheme !== "file" || parsed.authority || parsed.query || parsed.fragment || !parsed.path.startsWith("/work/")) throw new Error(i18n.t("projects:workspaceEdit.outsideProject"));
    const path = parsed.path.slice(6);
    if (!path || path.split("/").some(part => !part || part === "." || part === ".." || part === ".git")) throw new Error(i18n.t("projects:workspaceEdit.invalidPath"));
    if (seen.has(path)) throw new Error(i18n.t("projects:workspaceEdit.duplicateFile"));
    seen.add(path);
    if (version !== null && (!Number.isSafeInteger(version) || version < 0)) throw new Error(i18n.t("projects:workspaceEdit.invalidVersion"));
    if (!Array.isArray(edits) || edits.length > 10000) throw new Error(i18n.t("projects:workspaceEdit.invalidEdits"));
    return { path, version, edits: edits as TextEdit[] };
  });
}

/** Apply UTF-16 LSP ranges against the exact source, rejecting invalid or overlapping edits. */
export function applyTextEdits(source: string, edits: TextEdit[]): string {
  const model = monaco.editor.createModel(source);
  try {
    const normalized = model.getValue();
    const offsets = edits.map(edit => {
      if (!edit || typeof edit.newText !== "string" || !edit.range) throw new Error(i18n.t("projects:workspaceEdit.invalidEdits"));
      const offset = (position: Position) => {
        if (!position || !Number.isSafeInteger(position.line) || !Number.isSafeInteger(position.character) || position.line < 0 || position.character < 0) throw new Error(i18n.t("projects:workspaceEdit.invalidPosition"));
        const requested = { lineNumber: position.line + 1, column: position.character + 1 };
        const valid = model.validatePosition(requested);
        if (valid.lineNumber !== requested.lineNumber || valid.column !== requested.column) throw new Error(i18n.t("projects:workspaceEdit.outOfRange"));
        return model.getOffsetAt(valid);
      };
      const start = offset(edit.range.start), end = offset(edit.range.end);
      if (start > end) throw new Error(i18n.t("projects:workspaceEdit.invalidRange"));
      return { start, end, text: edit.newText };
    }).sort((a, b) => a.start - b.start || a.end - b.end);
    for (let i = 1; i < offsets.length; i++) if (offsets[i].start < offsets[i - 1].end || offsets[i].start === offsets[i - 1].start) throw new Error(i18n.t("projects:workspaceEdit.overlapping"));
    return offsets.reverse().reduce((text, edit) => text.slice(0, edit.start) + edit.text + text.slice(edit.end), normalized);
  } finally { model.dispose(); }
}
