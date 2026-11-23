import { useEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { gatewayRequest } from "@/gateway";
import { Button } from "@/components/ui/button";
import type { IndexStatus } from "./bib-search-model";

export function SearchIndexStatus({ onSourceChange, onEngine }: { onSourceChange: () => void; onEngine: (engine: string) => void }) {
  const { t } = useTranslation("bibliography");
  const [status, setStatus] = useState<IndexStatus | null>(null);
  const [error, setError] = useState("");
  const [queued, setQueued] = useState(false);
  const source = useRef<number | null>(null);
  const callbacks = useRef({ onSourceChange, onEngine });
  callbacks.current = { onSourceChange, onEngine };
  useEffect(() => {
    const abort = new AbortController();
    let timer: number | undefined;
    const poll = async () => {
      let building = false;
      try {
        const result: IndexStatus = await gatewayRequest("GET", "/bib/search/status", undefined, { signal: abort.signal });
        if (typeof result.source_revision === "number") {
          if (source.current !== null && source.current !== result.source_revision) callbacks.current.onSourceChange();
          source.current = result.source_revision;
          setStatus(result); setError(""); building = result.building;
          callbacks.current.onEngine(result.engine);
        }
      } catch (failure) { if (!abort.signal.aborted) setError(String(failure)); }
      if (!abort.signal.aborted) timer = window.setTimeout(poll, building ? 500 : 3000);
    };
    void poll();
    return () => { abort.abort(); window.clearTimeout(timer); };
  }, []);
  const rebuild = async () => {
    setQueued(true); setError("");
    try { const result: IndexStatus = await gatewayRequest("POST", "/bib/search/rebuild"); setStatus(result); }
    catch (failure) { setError(String(failure)); } finally { setQueued(false); }
  };
  if (!status && !error) return null;
  return <details className="mt-3 border-t px-1 pt-2 text-xs">
    <summary className="cursor-pointer text-muted-foreground">{status?.building ? t("searchIndex.indexing", { count: status.processed_documents.toLocaleString() }) : status?.last_error ? t("searchIndex.needsAttention") : status?.pending_articles ? t("searchIndex.awaiting", { count: status.pending_articles }) : status?.engine === "legacy" ? t("searchIndex.basicEnabled") : t("searchIndex.ready")}</summary>
    <div className="space-y-1 py-2 text-muted-foreground">
      {status && <p>{t("searchIndex.indexedStats", { indexed: status.indexed_documents.toLocaleString(), pending: status.pending_articles.toLocaleString() })}</p>}
      {status?.building && <p>{t("searchIndex.buildingNote")}</p>}
      {status && !status.durable && status.engine !== "legacy" && <p>{t("searchIndex.temporaryNote")}</p>}
      {status?.last_error && <p role="alert" className="text-destructive">{status.last_error}</p>}
      {error && <p role="alert" className="text-destructive">{error}</p>}
      {status?.engine !== "legacy" && <Button size="xs" variant="secondary" disabled={queued || status?.building} onClick={() => void rebuild()}>{queued ? t("searchIndex.queuing") : t("searchIndex.rebuild")}</Button>}
    </div>
  </details>;
}
