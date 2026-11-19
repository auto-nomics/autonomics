/**
 * Bibliography record opener bridge: the workbench registers the real opener
 * (focuses the library tab and selects the record) at startup; agent resource
 * links call `openBibRecordEditor` without knowing about tabs.
 */

export type BibRecordEditorSpec = {
  articleId: string;
};

type BibRecordOpener = (spec: BibRecordEditorSpec) => void;

let opener: BibRecordOpener | null = null;

export function registerBibRecordOpener(implementation: BibRecordOpener): void {
  opener = implementation;
}

export function openBibRecordEditor(spec: BibRecordEditorSpec): void {
  opener?.(spec);
}
