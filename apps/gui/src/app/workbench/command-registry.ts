/**
 * Command registry + palette state: the single hub behind the command
 * center (Ctrl+P / Ctrl+Shift+P). Owners (shell, dialogs, editor kinds)
 * register/revoke command groups on mount; the palette merges them with
 * static shell commands.
 */

import { useSyncExternalStore } from "react";

export type WorkbenchCommand = {
  id: string;
  title: string;
  /** Short group label shown as a badge. */
  category?: string;
  keywords?: string;
  run: () => void;
};

const owners = new Map<string, WorkbenchCommand[]>();
const listeners = new Set<() => void>();
let version = 0;
// Cached merge so `allCommands()` returns a stable reference between
// mutations — useSyncExternalStore requires an invariant getSnapshot.
let cachedCommands: WorkbenchCommand[] = [];
let cachedVersion = -1;

function emit() {
  version += 1;
  for (const listener of listeners) listener();
}

export function registerCommands(owner: string, commands: WorkbenchCommand[]): () => void {
  owners.set(owner, commands);
  emit();
  return () => {
    owners.delete(owner);
    emit();
  };
}

/** Merged command list; order is owner registration order (stable). */
export function allCommands(): WorkbenchCommand[] {
  if (cachedVersion !== version) {
    const merged: WorkbenchCommand[] = [];
    for (const commands of owners.values()) merged.push(...commands);
    cachedCommands = merged;
    cachedVersion = version;
  }
  return cachedCommands;
}

export function subscribeCommands(listener: () => void): () => void {
  listeners.add(listener);
  return () => listeners.delete(listener);
}

export function getCommandsVersion(): number {
  return version;
}

/** `useSyncExternalStore`-compatible snapshot: the version counter keeps the
 *  identity stable across unrelated renders and changes only on edits. */
export function useAllCommands(): WorkbenchCommand[] {
  return useSyncExternalStore(subscribeCommands, allCommands, allCommands);
}

// ---------------------------------------------------------------------------
// Palette visibility (module store; opened from keybindings and the
// title-bar command center)
// ---------------------------------------------------------------------------

export type PaletteMode = "all" | "commands";

let palette: { open: boolean; mode: PaletteMode } = { open: false, mode: "all" };
const paletteListeners = new Set<() => void>();

export function subscribePalette(listener: () => void): () => void {
  paletteListeners.add(listener);
  return () => paletteListeners.delete(listener);
}

export function getPaletteSnapshot(): { open: boolean; mode: PaletteMode } {
  return palette;
}

export function usePaletteState(): { open: boolean; mode: PaletteMode } {
  return useSyncExternalStore(subscribePalette, getPaletteSnapshot);
}

export function openPalette(mode: PaletteMode = "all"): void {
  palette = { open: true, mode };
  for (const listener of paletteListeners) listener();
}

export function closePalette(): void {
  if (!palette.open) return;
  palette = { open: false, mode: palette.mode };
  for (const listener of paletteListeners) listener();
}
