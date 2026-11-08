import { useEffect, useRef, useState } from "react";
import * as monaco from "monaco-editor";
import "@/components/editor/monaco-workers";
import { useTheme } from "@/hooks/use-theme";
import { LanguageClient } from "./language-client";
import type { FileEdits } from "./workspace-edit";

// Project TypeScript intelligence comes from the development container, not
// Monaco's browser worker, which cannot see the project's dependencies.
const containerLanguageMode = { completionItems: false, hovers: false, documentSymbols: false, definitions: false, references: false, documentHighlights: false, rename: false, diagnostics: false, documentRangeFormattingEdits: false, signatureHelp: false, onTypeFormattingEdits: false, codeActions: false, inlayHints: false };
monaco.languages.typescript.typescriptDefaults.setModeConfiguration(containerLanguageMode);
monaco.languages.typescript.javascriptDefaults.setModeConfiguration(containerLanguageMode);

export function CodeEditor({ project, path, value, readOnly = false, lspEnabled = true, saveVersion = 0, navigation, breakpoints = [], onToggleBreakpoint, onNavigate, onChange, onSave, onApplyEdits }: { project: string; path: string; value: string; readOnly?: boolean; lspEnabled?: boolean; saveVersion?: number; navigation?: { path: string; line: number; column: number; id: number }; breakpoints?: number[]; onToggleBreakpoint?: (line: number) => void; onNavigate: (path: string, line: number, column: number) => Promise<boolean>; onChange: (value: string) => void; onSave: () => void; onApplyEdits: (edits: FileEdits[], valid: () => boolean) => Promise<string | undefined> }) {
  const host = useRef<HTMLDivElement>(null);
  const editorRef = useRef<monaco.editor.IStandaloneCodeEditor | null>(null);
  const languageRef = useRef<LanguageClient | null>(null);
  const callbacks = useRef({ onChange, onSave, onNavigate, onToggleBreakpoint, onApplyEdits });
  callbacks.current = { onChange, onSave, onNavigate, onToggleBreakpoint, onApplyEdits };
  const { resolved } = useTheme();
  const [languageStatus, setLanguageStatus] = useState("");
  const [languageRetry, setLanguageRetry] = useState(0);

  useEffect(() => {
    if (!host.current) return;
    const uri = monaco.Uri.from({ scheme: "inmemory", authority: "plugin-projects", path: `/${project}/${path}` });
    const model = monaco.editor.createModel(value, path.toLowerCase().endsWith(".r") ? "r" : undefined, uri);
    const editor = monaco.editor.create(host.current, {
      model,
      automaticLayout: true,
      minimap: { enabled: false },
      glyphMargin: true,
      wordBasedSuggestions: "off",
      fontSize: 14,
      scrollBeyondLastLine: false,
      ariaLabel: `编辑 ${path}`,
    });
    editorRef.current = editor;
    const gutter = editor.onMouseDown(event => {
      if (event.target.type === monaco.editor.MouseTargetType.GUTTER_GLYPH_MARGIN && event.target.position) callbacks.current.onToggleBreakpoint?.(event.target.position.lineNumber);
    });
    const subscription = model.onDidChangeContent(() => callbacks.current.onChange(model.getValue()));
    const opener = monaco.editor.registerEditorOpener({ openCodeEditor: (source, resource, target) => {
      const prefix = `/${project}/`;
      if (source !== editor || resource.scheme !== "inmemory" || resource.authority !== "plugin-projects" || !resource.path.startsWith(prefix)) return false;
      const line = target ? ("startLineNumber" in target ? target.startLineNumber : target.lineNumber) : 1;
      const column = target ? ("startColumn" in target ? target.startColumn : target.column) : 1;
      return callbacks.current.onNavigate(resource.path.slice(prefix.length), line, column);
    } });
    editor.addCommand(monaco.KeyMod.CtrlCmd | monaco.KeyCode.KeyS, () => callbacks.current.onSave());
    editor.addCommand(monaco.KeyCode.F2, () => {
      const position = editor.getPosition();
      if (!position || editor.getOption(monaco.editor.EditorOption.readOnly)) return;
      const name = window.prompt("输入新的符号名称", model.getWordAtPosition(position)?.word ?? "");
      if (name?.trim()) void languageRef.current?.rename(position, name);
    });
    return () => { gutter.dispose(); opener.dispose(); subscription.dispose(); editor.dispose(); model.dispose(); editorRef.current = null; };
  // Each project/path owns a model; changes to its text are handled below.
  // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [project, path]);

  useEffect(() => {
    const model = editorRef.current?.getModel();
    if (model && model.getValue() !== value) { const editor = editorRef.current; editor?.pushUndoStop(); model.pushEditOperations([], [{ range: model.getFullModelRange(), text: value }], () => null); editor?.pushUndoStop(); }
  }, [value]);
  useEffect(() => { monaco.editor.setTheme(resolved === "dark" ? "vs-dark" : "vs"); }, [resolved]);
  useEffect(() => { editorRef.current?.updateOptions({ readOnly }); }, [readOnly, project, path]);
  useEffect(() => {
    const editor = editorRef.current;
    if (!editor) return;
    const decorations = editor.createDecorationsCollection(breakpoints.map(line => ({ range: new monaco.Range(line, 1, line, 1), options: { glyphMarginClassName: "project-breakpoint", glyphMarginHoverMessage: { value: "断点" } } })));
    return () => decorations.clear();
  }, [project, path, breakpoints]);
  useEffect(() => {
    const model = editorRef.current?.getModel();
    const language = model?.getLanguageId();
    if (!model || !lspEnabled || !language || !["python", "r", "rust", "typescript", "javascript", "c", "cpp", "go"].includes(language)) { setLanguageStatus(""); return; }
    const client = new LanguageClient(project, language, path, model, setLanguageStatus, async (edits, valid) => {
      const updated = await callbacks.current.onApplyEdits(edits, valid);
      if (updated !== undefined && !model.isDisposed() && model.getValue() !== updated) {
        editorRef.current?.pushUndoStop();
        model.pushEditOperations([], [{ range: model.getFullModelRange(), text: updated }], () => null);
        editorRef.current?.pushUndoStop();
      }
    });
    languageRef.current = client;
    void client.start();
    return () => { languageRef.current = null; client.dispose(); };
  }, [project, path, lspEnabled, languageRetry]);
  useEffect(() => { if (saveVersion) languageRef.current?.saved(); }, [saveVersion]);
  useEffect(() => { if (navigation?.path === path && editorRef.current) { editorRef.current.setPosition({ lineNumber: navigation.line, column: navigation.column }); editorRef.current.revealLineInCenter(navigation.line); editorRef.current.focus(); } }, [navigation, path]);

  return <div className="flex min-h-0 min-w-0 flex-1 flex-col"><style>{`.project-breakpoint { background: #e5484d; border-radius: 50%; width: 12px !important; height: 12px !important; margin: 4px; }`}</style><div ref={host} className="min-h-0 min-w-0 flex-1" />{languageStatus && <div role="status" className="flex shrink-0 items-center gap-2 border-t px-3 py-1 text-xs text-muted-foreground"><span>{languageStatus}</span>{languageStatus.startsWith("语言服务不可用") && <button type="button" className="underline" onClick={() => setLanguageRetry(value => value + 1)}>重试</button>}</div>}</div>;
}
