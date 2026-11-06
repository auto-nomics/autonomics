/**
 * VFS file opener: routes a virtual path to the right tab surface.
 * Text files open the editable `vfs-file` editor; images open the same
 * kind in preview mode; everything else downloads. Read-only mounts
 * render the editor read-only with a download button.
 */

import { openEditor } from "@/app/workbench/workbench-store";
import { downloadVfsFile, isImagePath, isTextPath } from "./vfs-api";

export function openVfsFile(path: string, readOnly = false): void {
  openEditor({ kind: "vfs-file", params: { path, readOnly } });
}

/** Non-text, non-image entry: download straight from the tree. */
export async function openOrDownloadVfsFile(path: string, readOnly = false): Promise<void> {
  if (isTextPath(path) || isImagePath(path)) {
    openVfsFile(path, readOnly);
    return;
  }
  await downloadVfsFile(path);
}

export { downloadVfsFile, isImagePath, isTextPath };
