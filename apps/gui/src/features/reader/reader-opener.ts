/**
 * Reader opener bridge: the workbench registers the real opener (opens an
 * editor tab per reader) at startup; bibliography code calls
 * `openReaderEditor` without knowing about tabs. When no opener is
 * registered (feature mount harnesses, CI), callers fall back to hosting
 * the reader directly — check `hasReaderOpener`.
 */

import type { ReaderSearchTarget } from "./reader-target";

export type ReaderEditorSpec = {
  articleId: string;
  title: string;
  attachmentKey?: string | null;
  expectedHash?: string | null;
  search?: ReaderSearchTarget;
};

type ReaderOpener = (spec: ReaderEditorSpec) => void;

let opener: ReaderOpener | null = null;

export function registerReaderOpener(implementation: ReaderOpener): void {
  opener = implementation;
}

export function hasReaderOpener(): boolean {
  return opener !== null;
}

export function openReaderEditor(spec: ReaderEditorSpec): void {
  opener?.(spec);
}
