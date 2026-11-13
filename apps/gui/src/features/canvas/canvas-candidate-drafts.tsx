import { createContext, useContext, useRef, useState, type Dispatch, type ReactNode, type SetStateAction } from "react";
type Drafts = { scope: string; values: Map<string, unknown> };
const Context = createContext<Drafts | null>(null);
export function CanvasCandidateDraftProvider({ scope, children }: { scope: string; children: ReactNode }) {
  const values = useRef(new Map<string, unknown>());
  return <Context.Provider value={{ scope, values: values.current }}>{children}</Context.Provider>;
}
// Only user-entered drafts live here. Immutable candidate hashes prevent replay
// into another version; the provider's lifetime is the current canvas page.
export function useCandidateDraft<T>(candidate: { id: string; document_sha256?: string }, field: readonly string[], initial: T): [T, Dispatch<SetStateAction<T>>, () => void] {
  const drafts = useContext(Context);
  if (!drafts) throw new Error("Candidate draft provider is missing");
  const key = JSON.stringify([drafts.scope, candidate.id, candidate.document_sha256, field]);
  const baseline = useRef(initial);
  const [value, setValue] = useState<T>(() => drafts.values.has(key) ? drafts.values.get(key) as T : initial);
  const current = useRef(value);
  const update: Dispatch<SetStateAction<T>> = next => {
    const resolved = typeof next === "function" ? (next as (previous: T) => T)(current.current) : next;
    current.current = resolved; drafts.values.set(key, resolved); setValue(resolved);
  };
  const reset = () => { drafts.values.delete(key); current.current = baseline.current; setValue(baseline.current); };
  return [value, update, reset];
}
