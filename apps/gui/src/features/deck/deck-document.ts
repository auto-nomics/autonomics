import { useMemo, useSyncExternalStore } from "react";
import { fetchDeck, fetchDeckSource, saveDeckSource, type DeckPayload } from "./deck-api";
import { setDocumentDirty } from "@/app/workbench/dirty-documents";

type Snapshot = { source: string | null; savedSource: string; sourceRevision: string; sourceError: string | null; sourceLoading: boolean; saving: boolean; payload: DeckPayload | null; loading: boolean; error: string | null };

/** One resource model per deck; views own navigation and presentation state. */
class DeckDocument {
  snapshot: Snapshot = { source: null, savedSource: "", sourceRevision: "", sourceError: null, sourceLoading: false, saving: false, payload: null, loading: true, error: null };
  listeners = new Set<() => void>();
  private pending: Promise<DeckPayload> | null = null;
  private timer: ReturnType<typeof setInterval> | undefined;
  constructor(readonly id: string) {}
  private publish(snapshot: Snapshot) {
    this.snapshot = snapshot;
    setDocumentDirty(`deck:${this.id}`, snapshot.saving || (snapshot.source !== null && snapshot.source !== snapshot.savedSource));
    for (const listener of this.listeners) listener();
  }
  refresh = (): Promise<DeckPayload> => {
    if (this.pending) return this.pending;
    this.pending = fetchDeck(this.id).then(payload => {
      const previous = this.snapshot.payload;
      if (!previous || previous.deck.revision !== payload.deck.revision || previous.themeRevision !== payload.themeRevision || this.snapshot.error) {
        this.publish({ ...this.snapshot, payload, loading: false, error: null });
      }
      return payload;
    }).catch(error => {
      // A transient refresh failure keeps the last usable document visible.
      if (!this.snapshot.payload) this.publish({ ...this.snapshot, payload: null, loading: false, error: String(error?.message ?? error) });
      throw error;
    }).finally(() => { this.pending = null; });
    return this.pending;
  };
  setSource = (source: string) => this.publish({ ...this.snapshot, source });
  discardSource = () => this.publish({ ...this.snapshot, source: this.snapshot.savedSource, sourceError: null });
  loadSource = async () => {
    if (this.snapshot.sourceLoading || this.snapshot.source !== null) return;
    this.publish({ ...this.snapshot, sourceLoading: true, sourceError: null });
    try {
      const result = await fetchDeckSource(this.id);
      this.publish({ ...this.snapshot, source: result.markdown, savedSource: result.markdown, sourceRevision: result.revision });
    } catch (error) {
      this.publish({ ...this.snapshot, sourceError: String(error instanceof Error ? error.message : error) });
    } finally { this.publish({ ...this.snapshot, sourceLoading: false }); }
  };
  saveSource = async () => {
    const { source, sourceRevision } = this.snapshot;
    if (source === null || this.snapshot.saving) return;
    this.publish({ ...this.snapshot, saving: true, sourceError: null });
    try {
      await saveDeckSource(this.id, source, sourceRevision);
      const result = await fetchDeckSource(this.id);
      this.publish({ ...this.snapshot, source: this.snapshot.source === source ? result.markdown : this.snapshot.source, savedSource: result.markdown, sourceRevision: result.revision });
      await this.refresh();
    } catch (error) {
      this.publish({ ...this.snapshot, sourceError: `保存失败：${String(error instanceof Error ? error.message : error)}。如有版本冲突，请复制草稿后重新打开文稿。` });
    } finally { this.publish({ ...this.snapshot, saving: false }); }
  };
  private poll = () => {
    if (document.visibilityState === "visible") void this.refresh().catch(() => {});
  };
  subscribe = (listener: () => void) => {
    this.listeners.add(listener);
    if (this.listeners.size === 1) {
      void this.refresh().catch(() => {});
      this.timer = setInterval(this.poll, 4000);
      document.addEventListener("visibilitychange", this.poll);
    }
    return () => {
      this.listeners.delete(listener);
      if (!this.listeners.size) {
        clearInterval(this.timer);
        document.removeEventListener("visibilitychange", this.poll);
        // React can unsubscribe and resubscribe in one commit. Defer disposal
        // so that this still counts as one live resource model.
        queueMicrotask(() => {
          if (!this.listeners.size && (this.snapshot.source === null || this.snapshot.source === this.snapshot.savedSource) && documents.get(this.id) === this) documents.delete(this.id);
        });
      }
    };
  };
  getSnapshot = () => this.snapshot;
}

const documents = new Map<string, DeckDocument>();
/** Resource consumers (including agent tools) use the same document model. */
export function getDeckDocument(id: string): DeckDocument {
  let model = documents.get(id);
  if (!model) { model = new DeckDocument(id); documents.set(id, model); }
  return model;
}
export function useDeckDocument(id: string) {
  const model = useMemo(() => getDeckDocument(id), [id]);
  const snapshot = useSyncExternalStore(model.subscribe, model.getSnapshot);
  return { ...snapshot, refresh: model.refresh, setSource: model.setSource, loadSource: model.loadSource, saveSource: model.saveSource };
}
