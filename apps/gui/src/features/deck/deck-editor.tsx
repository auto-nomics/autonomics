/**
 * DeckEditorBody — the workbench editor body for `deck:{deckId}` tabs:
 * fetches the deck payload, renders DeckRenderer, handles load states.
 *
 * Live updates: while the tab is mounted (and the document visible) the
 * payload is re-fetched every LIVE_POLL_MS and swapped in when the
 * server revision changes — agent edits (slide_patch / insert / …) show
 * up in the open tab within a poll interval. DeckRenderer keeps its
 * navigation state across swaps, so the viewer stays on the current
 * slide. Per-tab state (last slide) lives in `editor.state` via
 * patchEditorState.
 */
import { useEffect, useRef, useState } from "react";
import { Loader2, RefreshCw } from "lucide-react";
import { fetchDeck, type DeckPayload } from "./deck-api";
import { DeckRenderer } from "./render/deck-renderer";

const LIVE_POLL_MS = 4000;

export interface DeckEditorBodyProps {
  deckId: string;
  startSlide?: number;
  onSlideSeen?: (no: number) => void;
}

export function DeckEditorBody({ deckId, startSlide, onSlideSeen }: DeckEditorBodyProps): React.ReactNode {
  const [payload, setPayload] = useState<DeckPayload | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [loading, setLoading] = useState(true);
  const [reloadTick, setReloadTick] = useState(0);
  const revisionRef = useRef<string | null>(null);

  useEffect(() => {
    let cancelled = false;
    setLoading(true);
    setError(null);
    revisionRef.current = null;
    fetchDeck(deckId)
      .then(p => {
        if (!cancelled) {
          revisionRef.current = p.deck.revision;
          setPayload(p);
          setLoading(false);
        }
      })
      .catch(e => {
        if (!cancelled) {
          setError(String(e?.message ?? e));
          setLoading(false);
        }
      });
    return () => {
      cancelled = true;
    };
  }, [deckId, reloadTick]);

  // Live sync: poll while visible; swap on revision change. Poll errors
  // are transient (daemon restart etc.) — keep the last good payload.
  useEffect(() => {
    let cancelled = false;
    let inFlight = false;
    const tick = async () => {
      if (cancelled || inFlight || document.visibilityState !== "visible") return;
      inFlight = true;
      try {
        const next = await fetchDeck(deckId);
        if (cancelled) return;
        if (next.deck.revision !== revisionRef.current) {
          revisionRef.current = next.deck.revision;
          setPayload(next);
        }
      } catch {
        // keep last good payload; retry next tick
      } finally {
        inFlight = false;
      }
    };
    const timer = window.setInterval(tick, LIVE_POLL_MS);
    const onVisible = () => {
      if (document.visibilityState === "visible") void tick();
    };
    document.addEventListener("visibilitychange", onVisible);
    return () => {
      cancelled = true;
      window.clearInterval(timer);
      document.removeEventListener("visibilitychange", onVisible);
    };
  }, [deckId]);

  const dark = useDeckDark();

  void onSlideSeen; // wired through the workbench wrapper; renderer owns navigation.

  if (loading) {
    return (
      <div className="deck-editor deck-editor--loading">
        <Loader2 className="spin" size={18} />
        <span>加载 deck…</span>
      </div>
    );
  }
  if (error || !payload) {
    return (
      <div className="deck-editor deck-editor--error">
        <span>加载失败:{error ?? "未知错误"}</span>
        <button type="button" className="deck-btn" onClick={() => setReloadTick(t => t + 1)}>
          <RefreshCw size={14} /> 重试
        </button>
      </div>
    );
  }

  return (
    <div className="deck-editor">
      <DeckRenderer payload={payload} dark={dark} startSlide={startSlide} />
    </div>
  );
}

/** Follow the app theme (light/dark persisted by initTheme). */
function useDeckDark(): boolean {
  const [dark, setDark] = useState<boolean>(() => detectDark());
  useEffect(() => {
    const observer = new MutationObserver(() => setDark(detectDark()));
    observer.observe(document.documentElement, { attributes: true, attributeFilter: ["class", "data-theme"] });
    return () => observer.disconnect();
  }, []);
  return dark;
}

function detectDark(): boolean {
  const attr = document.documentElement.getAttribute("data-theme");
  if (attr === "dark") return true;
  if (attr === "light") return false;
  return document.documentElement.classList.contains("dark");
}
