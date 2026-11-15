import { useEffect, useRef } from "react";
import * as monaco from "monaco-editor";
import "@/components/editor/monaco-workers";
import { useTheme } from "@/hooks/use-theme";

if (
  !monaco.languages.getLanguages().some((language) => language.id === "latex")
) {
  monaco.languages.register({ id: "latex", extensions: [".tex", ".bib"] });
  monaco.languages.setMonarchTokensProvider("latex", {
    tokenizer: {
      root: [
        [/%.*/, "comment"],
        [/\\[a-zA-Z@]+\*?/, "keyword"],
        [/[{}]/, "delimiter.bracket"],
        [/\$[^$]*\$/, "string"],
        [/@[a-zA-Z]+/, "keyword"],
      ],
    },
  });
  monaco.languages.setLanguageConfiguration("latex", {
    comments: { lineComment: "%" },
    brackets: [
      ["{", "}"],
      ["[", "]"],
      ["(", ")"],
    ],
    autoClosingPairs: [
      { open: "{", close: "}" },
      { open: "[", close: "]" },
      { open: "(", close: ")" },
    ],
  });
}
export type SourceSelection = {
  startLine: number;
  endLine: number;
  text: string;
};
export function SourceEditor({
  path,
  value,
  onChange,
  onSave,
  onSelection,
  navigation,
  insertion,
}: {
  path: string;
  value: string;
  onChange: (value: string) => void;
  onSave: () => void;
  onSelection: (selection: SourceSelection) => void;
  navigation?: { line: number; id: number };
  insertion?: { text: string; id: number };
}) {
  const host = useRef<HTMLDivElement>(null),
    editorRef = useRef<monaco.editor.IStandaloneCodeEditor | null>(null);
  const callbacks = useRef({ onChange, onSave, onSelection });
  callbacks.current = { onChange, onSave, onSelection };
  const { resolved } = useTheme();
  useEffect(() => {
    const model = monaco.editor.createModel(value, "latex");
    const editor = monaco.editor.create(host.current!, {
      model,
      automaticLayout: true,
      occurrencesHighlight: "off",
      selectionHighlight: false,
      minimap: { enabled: false },
      fontSize: 13,
      lineHeight: 22,
      wordWrap: "on",
      scrollBeyondLastLine: false,
      padding: { top: 18, bottom: 18 },
      ariaLabel: `LaTeX source ${path}`,
      tabSize: 2,
      quickSuggestions: false,
      wordBasedSuggestions: "off",
    });
    editorRef.current = editor;
    const changed = model.onDidChangeContent(() =>
      callbacks.current.onChange(model.getValue()),
    );
    const selected = editor.onDidChangeCursorSelection((event) =>
      callbacks.current.onSelection({
        startLine: event.selection.startLineNumber,
        endLine: event.selection.endLineNumber,
        text: model.getValueInRange(event.selection),
      }),
    );
    editor.addCommand(monaco.KeyMod.CtrlCmd | monaco.KeyCode.KeyS, () =>
      callbacks.current.onSave(),
    );
    return () => {
      changed.dispose();
      selected.dispose();
      editor.dispose();
      model.dispose();
      editorRef.current = null;
    };
    // Text updates are synchronized below, while changing files creates a fresh editor.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [path]);
  useEffect(() => {
    const model = editorRef.current?.getModel();
    if (model && model.getValue() !== value) {
      model.pushEditOperations(
        [],
        [{ range: model.getFullModelRange(), text: value }],
        () => null,
      );
    }
  }, [value]);
  useEffect(() => {
    monaco.editor.setTheme(resolved === "dark" ? "vs-dark" : "vs");
  }, [resolved]);
  useEffect(() => {
    if (navigation) {
      editorRef.current?.setPosition({
        lineNumber: navigation.line,
        column: 1,
      });
      editorRef.current?.revealLineInCenter(navigation.line);
      editorRef.current?.focus();
    }
  }, [navigation]);
  useEffect(() => {
    const editor = editorRef.current;
    if (editor && insertion) {
      editor.executeEdits("citation", [
        {
          range: editor.getSelection()!,
          text: insertion.text,
          forceMoveMarkers: true,
        },
      ]);
      editor.focus();
    }
  }, [insertion]);
  return <div className="manuscript-monaco" ref={host} />;
}
export function ProposalDiff({
  original,
  proposed,
}: {
  original: string;
  proposed: string;
}) {
  const host = useRef<HTMLDivElement>(null);
  const models = useRef<{
    before: monaco.editor.ITextModel;
    after: monaco.editor.ITextModel;
  } | null>(null);
  useEffect(() => {
    const before = monaco.editor.createModel(original, "latex"),
      after = monaco.editor.createModel(proposed, "latex");
    models.current = { before, after };
    const editor = monaco.editor.createDiffEditor(host.current!, {
      automaticLayout: true,
      occurrencesHighlight: "off",
      selectionHighlight: false,
      readOnly: true,
      renderSideBySide: false,
      wordWrap: "on",
      diffWordWrap: "on",
      minimap: { enabled: false },
      fontSize: 12,
      scrollBeyondLastLine: false,
      ariaLabel: "Proposed changes",
    });
    editor.setModel({ original: before, modified: after });
    const changed = editor.onDidUpdateDiff(() => {
      const first = editor.getLineChanges()?.[0];
      if (first)
        editor
          .getModifiedEditor()
          .revealLineInCenter(Math.max(1, first.modifiedStartLineNumber));
    });
    return () => {
      changed.dispose();
      editor.setModel(null);
      editor.dispose();
      models.current = null;
      before.dispose();
      after.dispose();
    };
    // Models persist while conflict drafts change, avoiding disposed-worker races.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);
  useEffect(() => {
    if (models.current) {
      if (models.current.before.getValue() !== original)
        models.current.before.setValue(original);
      if (models.current.after.getValue() !== proposed)
        models.current.after.setValue(proposed);
    }
  }, [original, proposed]);
  return (
    <div ref={host} className="manuscript-diff" aria-label="Proposed changes" />
  );
}
