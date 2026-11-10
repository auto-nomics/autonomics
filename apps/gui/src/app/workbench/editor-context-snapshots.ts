import { useEffect, useRef } from "react";

type Snapshot = Record<string, string | number | boolean | null>;
const providers = new Map<string, () => Snapshot>();

/** Live drafts stay in memory, outside the persisted workbench layout. */
export function useEditorContextSnapshot(id: string, snapshot: Snapshot) {
  const latest = useRef(snapshot);
  latest.current = snapshot;
  useEffect(() => {
    const read = () => latest.current;
    providers.set(id, read);
    return () => { if (providers.get(id) === read) providers.delete(id); };
  }, [id]);
}

export function editorContextSnapshot(id: string) { return providers.get(id)?.() ?? null; }

export function boundedContextText(content: string) {
  const limit = 24_000;
  return { content: content.slice(0, limit), truncated: content.length > limit };
}
