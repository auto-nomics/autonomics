import { useMemo, useSyncExternalStore } from "react";
import type { WikiEntry } from "./wiki-api";
import { setDocumentDirty } from "@/app/workbench/dirty-documents";

type Draft = { title: string; alias: string; tags: string; content: string };
const fields = (entry: WikiEntry): Draft => ({ title: entry.title, alias: entry.alias.join(", "), tags: entry.tags.join(", "), content: entry.content });
class WikiDocument {
  state: Draft & { entry: WikiEntry; saving: boolean };
  listeners = new Set<() => void>();
  constructor(entry: WikiEntry) { this.state = { ...fields(entry), entry, saving: false }; }
  get dirty() {
    const baseline = fields(this.state.entry);
    return (Object.keys(baseline) as (keyof Draft)[]).some(key => baseline[key] !== this.state[key]);
  }
  private emit() {
    setDocumentDirty(`wiki:${this.state.entry.id}`, this.dirty || this.state.saving);
    for (const listener of this.listeners) listener();
  }
  update = (patch: Partial<Draft> & { saving?: boolean }) => { this.state = { ...this.state, ...patch }; this.emit(); };
  accept = (entry: WikiEntry, sent: Draft) => {
    const next = fields(entry);
    for (const key of Object.keys(next) as (keyof Draft)[]) if (this.state[key] !== sent[key]) next[key] = this.state[key];
    this.state = { ...this.state, ...next, entry };
    this.emit();
  };
  subscribe = (listener: () => void) => {
    this.listeners.add(listener);
    return () => {
      this.listeners.delete(listener);
      queueMicrotask(() => {
        if (!this.listeners.size && !this.dirty && !this.state.saving && documents.get(this.state.entry.id) === this) documents.delete(this.state.entry.id);
      });
    };
  };
  snapshot = () => this.state;
}
const documents = new Map<string, WikiDocument>();
export function findWikiDocument(id: string) { return documents.get(id); }
export function useWikiDocument(entry: WikiEntry) {
  const model = useMemo(() => {
    let document = documents.get(entry.id);
    if (!document) { document = new WikiDocument(entry); documents.set(entry.id, document); }
    return document;
  }, [entry.id]);
  const state = useSyncExternalStore(model.subscribe, model.snapshot);
  return { ...state, model };
}
