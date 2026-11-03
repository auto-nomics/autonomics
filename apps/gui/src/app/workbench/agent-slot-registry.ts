/** Agent editor slots host independent per-agent workspaces. */

import { useSyncExternalStore } from "react";

const slots = new Map<string, HTMLDivElement | null>();
const listeners = new Set<() => void>();
let version = 0;

function emit() {
  version += 1;
  for (const listener of listeners) listener();
}

export function setAgentSlot(path: string, node: HTMLDivElement | null): void {
  const current = slots.get(path) ?? null;
  if (current === node) return;
  if (node === null) slots.delete(path);
  else slots.set(path, node);
  emit();
}

/** useSyncExternalStore snapshot: the version keeps subscriptions honest
 *  while the returned element reference stays stable between changes. */
export function getAgentSlotVersion(): number {
  return version;
}

export function subscribeAgentSlots(listener: () => void): () => void {
  listeners.add(listener);
  return () => {
    listeners.delete(listener);
  };
}

export function getAgentSlot(path: string): HTMLDivElement | null {
  return slots.get(path) ?? null;
}

/** Reactive slot lookup for the shell. */
export function useAgentSlot(path: string): HTMLDivElement | null {
  useSyncExternalStore(subscribeAgentSlots, getAgentSlotVersion);
  return getAgentSlot(path);
}
