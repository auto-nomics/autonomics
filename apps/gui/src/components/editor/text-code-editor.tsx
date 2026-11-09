import { useEffect, useRef } from "react";
import * as monaco from "monaco-editor";
import "./monaco-workers";
import { useTheme } from "@/hooks/use-theme";

const documentModels = new Map<string, { model: monaco.editor.ITextModel; views: number }>();

function acquireModel(documentKey: string, language: string, value: string) {
  const key = JSON.stringify([documentKey, language]);
  let entry = documentModels.get(key);
  if (!entry) {
    entry = { model: monaco.editor.createModel(value, language), views: 0 };
    documentModels.set(key, entry);
  }
  entry.views++;
  const owned = entry;
  return { model: owned.model, release: () => {
    owned.views--;
    queueMicrotask(() => {
      if (owned.views || documentModels.get(key) !== owned) return;
      documentModels.delete(key);
      owned.model.dispose();
    });
  } };
}

/** Controlled Monaco editor for workbench documents without a container LSP. */
export function TextCodeEditor({ documentKey, language, value, readOnly = false, label, onChange, onSave }: {
  documentKey: string;
  language: "markdown" | "css";
  value: string;
  readOnly?: boolean;
  label: string;
  onChange: (value: string) => void;
  onSave: () => void;
}): React.ReactNode {
  const host = useRef<HTMLDivElement>(null);
  const editorRef = useRef<monaco.editor.IStandaloneCodeEditor | null>(null);
  const changing = useRef(false);
  const callbacks = useRef({ onChange, onSave });
  callbacks.current = { onChange, onSave };
  const { resolved } = useTheme();

  useEffect(() => {
    if (!host.current) return;
    // Editor instances own cursor/viewport state; the resource owns text
    // and its undo stack, shared across split views.
    const document = acquireModel(documentKey, language, value);
    const model = document.model;
    const editor = monaco.editor.create(host.current, {
      model, automaticLayout: true, minimap: { enabled: false },
      fontSize: 14, scrollBeyondLastLine: false, wordWrap: "on",
      // Monaco's cross-editor word-highlighter schedules a delayed query
      // whose cancellation escapes when a split view is disposed.
      occurrencesHighlight: "off",
      ariaLabel: label, readOnly,
    });
    editorRef.current = editor;
    const subscription = model.onDidChangeContent(() => {
      if (!changing.current) callbacks.current.onChange(model.getValue());
    });
    // Monaco's addCommand registers a global keybinding by default. Multiple
    // mounted editors would then route Ctrl+S to whichever registered last,
    // including a hidden CSS editor. Handle the shortcut on the focused
    // editor instead and dispose the listener with that instance.
    const keyboard = editor.onKeyDown(event => {
      if ((event.ctrlKey || event.metaKey) && event.keyCode === monaco.KeyCode.KeyS) {
        event.preventDefault(); event.stopPropagation(); callbacks.current.onSave();
      }
    });
    return () => { keyboard.dispose(); subscription.dispose(); editor.dispose(); document.release(); editorRef.current = null; };
    // Model identity changes only when switching documents or languages.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [documentKey, language]);

  useEffect(() => {
    const editor = editorRef.current;
    const model = editor?.getModel();
    if (!editor || !model || model.getValue() === value) return;
    changing.current = true;
    try {
      editor.pushUndoStop();
      model.pushEditOperations([], [{ range: model.getFullModelRange(), text: value }], () => null);
      editor.pushUndoStop();
    } finally { changing.current = false; }
  }, [value, documentKey, language]);
  useEffect(() => { editorRef.current?.updateOptions({ readOnly, ariaLabel: label }); }, [readOnly, label, documentKey, language]);
  useEffect(() => { monaco.editor.setTheme(resolved === "dark" ? "vs-dark" : "vs"); }, [resolved]);

  return <div ref={host} className="deck-monaco" data-code-editor={language} aria-label={label} />;
}
