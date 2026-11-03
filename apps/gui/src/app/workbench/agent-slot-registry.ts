/**
 * Agent slot registry: each agent editor tab body registers its slot DOM
 * node here; the single AgentWorkspace instance (rendered by the shell)
 * portals into the slot of the active agent. Tab switches move the portal
 * target — the workspace's transcript machinery is built to survive the
 * remount, and per-agent state lives in the runtime store.
 */

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
