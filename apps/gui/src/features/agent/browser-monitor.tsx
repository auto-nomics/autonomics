/**
 * Live browser monitor for browser-kind agents: polls
 * `GET /agents/{agent}/browser` and renders the active tab's viewport
 * screenshot plus the tab strip. Non-browser agents (or a dropped
 * session) show a quiet placeholder instead of an error.
 */

import { useCallback, useEffect, useRef, useState } from "react";
import { ExternalLink, Globe } from "lucide-react";
import { gatewayRequest } from "@/gateway";
import { cn } from "@/lib/utils";

const POLL_INTERVAL_MS = 2000;

type BrowserTab = { url?: string | null; title?: string | null };
type BrowserSnapshot = {
  live: boolean;
  active_index: number;
  tabs: BrowserTab[];
  screenshot?: string | null;
};

export type BrowserMonitorProps = {
  activeAgent: string;
  /** Only poll while the inspector is visible. */
  open: boolean;
};

export function BrowserMonitor({ activeAgent, open }: BrowserMonitorProps) {
  const [snapshot, setSnapshot] = useState<BrowserSnapshot | null>(null);
  const [failed, setFailed] = useState(false);
  const inFlight = useRef(false);

  const poll = useCallback(async () => {
    if (inFlight.current) return;
    inFlight.current = true;
    try {
      const data = await gatewayRequest("GET", `/agents/${encodeURIComponent(activeAgent)}/browser`);
      setSnapshot(data as BrowserSnapshot);
      setFailed(false);
    } catch {
      setFailed(true);
    } finally {
      inFlight.current = false;
    }
  }, [activeAgent]);

  useEffect(() => {
    if (!activeAgent || !open) return;
    void poll();
    const timer = setInterval(() => void poll(), POLL_INTERVAL_MS);
    return () => clearInterval(timer);
  }, [activeAgent, open, poll]);

  if (!activeAgent || !open) {
    return <div className="rounded-md border border-dashed p-2 text-xs text-muted-foreground">No agent selected</div>;
  }

  if (!snapshot?.live) {
    return (
      <div className="rounded-md border border-dashed p-2 text-xs text-muted-foreground">
        {failed ? "Browser monitor unavailable" : "No live browser session — browser agents only"}
      </div>
    );
  }

  const activeTab = snapshot.tabs[snapshot.active_index];
  const url = activeTab?.url ?? "";

  return (
    <div className="overflow-hidden rounded-lg border bg-card">
      {snapshot.screenshot ? (
        <img
          src={`data:image/png;base64,${snapshot.screenshot}`}
          alt="Active browser tab screenshot"
          className="w-full bg-white"
        />
      ) : (
        <div className="flex aspect-video items-center justify-center text-xs text-muted-foreground">
          <Globe className="mr-1.5 size-3.5" aria-hidden="true" /> No capture
        </div>
      )}
      {activeTab && (
        <div className="border-t px-2 py-1.5 text-[11px]">
          {activeTab.title && <div className="truncate font-medium" title={activeTab.title}>{activeTab.title}</div>}
          {url && (
            <a
              href={url}
              target="_blank"
              rel="noreferrer"
              className="mt-0.5 flex items-center gap-1 truncate text-muted-foreground hover:underline"
              title={url}
            >
              <ExternalLink className="size-3 shrink-0" aria-hidden="true" />
              <span className="truncate">{url}</span>
            </a>
          )}
        </div>
      )}
      {snapshot.tabs.length > 1 && (
        <div className="flex items-center gap-1 overflow-x-auto border-t px-1.5 py-1" aria-label="Open tabs">
          {snapshot.tabs.map((tab, index) => (
            <span
              key={index}
              title={tab.url ?? tab.title ?? `Tab ${index + 1}`}
              className={cn(
                "max-w-28 truncate rounded px-1.5 py-0.5 text-[10px]",
                index === snapshot.active_index ? "bg-primary/10 font-medium text-primary" : "text-muted-foreground",
              )}
            >
              {tab.title || tab.url || `Tab ${index + 1}`}
            </span>
          ))}
        </div>
      )}
    </div>
  );
}
