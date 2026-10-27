import { useEffect, useMemo, useState } from "react";
import { gatewayRequest } from "@/gateway";
import type { DiscoveryCandidate, DiscoveryPreview } from "./discovery-model";

type Presence = { key: string; label: string; error: boolean };
/** Match obtained pages in bounded batches; unknown/error never means absent.
 * Cache keys include the chosen source version and target library scope. */
export function useDiscoveryPresence(sessionId: string | undefined, candidates: DiscoveryCandidate[], collection: string, variants: Record<string, number>, source: string) {
  const [entries, setEntries] = useState<Record<string, Presence>>({});
  const [revision, setRevision] = useState(0);
  useEffect(() => setEntries({}), [sessionId, collection]);
  const selections = useMemo(() => candidates.map(candidate => {
    const record_index = variants[candidate.id] ?? Math.max(0, candidate.records.findIndex(record => record.source === source));
    return { candidate_id: candidate.id, expected_fingerprint: candidate.fingerprint, record_index, key: JSON.stringify([sessionId, collection, candidate.fingerprint, record_index, revision]) };
  }), [sessionId, candidates, collection, variants, source, revision]);
  useEffect(() => {
    if (!sessionId) return;
    const pending = selections.filter(item => entries[item.candidate_id]?.key !== item.key).slice(0, 100);
    if (!pending.length) return;
    const abort = new AbortController();
    const timer = setTimeout(() => {
      void gatewayRequest("POST", `/bib/search/sessions/${encodeURIComponent(sessionId)}/import/preview`, {
        selections: pending.map(({ candidate_id, expected_fingerprint, record_index }) => ({ candidate_id, expected_fingerprint, record_index })),
        ...(collection ? { collection_id: collection } : { library_id: 1 }),
      }, { signal: abort.signal }).then((response: { preview: DiscoveryPreview }) => {
        if (abort.signal.aborted) return;
        setEntries(current => {
          const next = { ...current };
          for (const item of pending) {
            const match = response.preview.items.find(match => match.candidate_id === item.candidate_id);
            next[item.candidate_id] = { key: item.key, label: !match || match.error ? "Review library match" : match.existing.length ? "Already in target library" : match.related_existing.length ? "Related version in target library" : "New to target library", error: !match || Boolean(match.error) };
          }
          return next;
        });
      }).catch(() => {
        if (abort.signal.aborted) return;
        setEntries(current => ({ ...current, ...Object.fromEntries(pending.map(item => [item.candidate_id, { key: item.key, label: "Library status unavailable · refresh to check again", error: true }])) }));
      });
    }, 250);
    return () => { clearTimeout(timer); abort.abort(); };
  }, [sessionId, selections, collection, entries]);
  const visible = useMemo(() => Object.fromEntries(selections.map(item => [item.candidate_id, entries[item.candidate_id]?.key === item.key ? entries[item.candidate_id] : { key: item.key, label: "Checking target library…", error: false }])), [entries, selections]);
  return { entries: visible, refresh: () => setRevision(value => value + 1) };
}
