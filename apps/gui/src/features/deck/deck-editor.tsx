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
import { useEffect, useId, useRef, useState } from "react";
import { Braces, FileCode2, Loader2, Presentation, RefreshCw, Save } from "lucide-react";
import { fetchDeck, fetchDeckSource, saveDeckSource, type DeckPayload } from "./deck-api";
import { DeckRenderer } from "./render/deck-renderer";
import { DeckCssEditor } from "./deck-css-editor";
import { TextCodeEditor } from "@/components/editor/text-code-editor";

const LIVE_POLL_MS = 4000;

export interface DeckEditorBodyProps {
  deckId: string;
  startSlide?: number;
  onSlideSeen?: (no: number) => void;
}

export function DeckEditorBody({ deckId, startSlide, onSlideSeen }: DeckEditorBodyProps): React.ReactNode {
  const tabId = useId();
  const [payload, setPayload] = useState<DeckPayload | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [loading, setLoading] = useState(true);
  const [reloadTick, setReloadTick] = useState(0);
  const revisionRef = useRef<string | null>(null);
  const payloadRef = useRef<DeckPayload | null>(null);
  payloadRef.current = payload;
  const [mode, setMode] = useState<"preview" | "source" | "css">("preview");
  const [currentSlide, setCurrentSlide] = useState(startSlide ?? 1);
  const [source, setSource] = useState<string | null>(null);
  const [savedSource, setSavedSource] = useState("");
  const [sourceRevision, setSourceRevision] = useState("");
  const [sourceError, setSourceError] = useState<string | null>(null);
  const [sourceLoading, setSourceLoading] = useState(false);
  const [saving, setSaving] = useState(false);
  useEffect(() => { setMode("preview"); setSource(null); setSavedSource(""); setSourceRevision(""); setSourceError(null); }, [deckId]);
  async function loadSource() {
    setSourceLoading(true); setSourceError(null);
    try { const result = await fetchDeckSource(deckId); setSource(result.markdown); setSavedSource(result.markdown); setSourceRevision(result.revision); }
    catch (e) { setSourceError(String(e instanceof Error ? e.message : e)); }
    finally { setSourceLoading(false); }
  }
  async function saveSource() {
    if (source === null) return;
    setSaving(true); setSourceError(null);
    try { await saveDeckSource(deckId, source, sourceRevision); const result = await fetchDeckSource(deckId); setSource(result.markdown); setSavedSource(result.markdown); setSourceRevision(result.revision); await refreshPayload(); }
    catch (e) { setSourceError(`保存失败：${String(e instanceof Error ? e.message : e)}。如有版本冲突，请复制草稿后重新打开文稿。`); }
    finally { setSaving(false); }
  }

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
        if (next.deck.revision !== revisionRef.current || next.themeRevision !== payloadRef.current?.themeRevision) {
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

  async function refreshPayload() {
    const next = await fetchDeck(deckId);
    revisionRef.current = next.deck.revision;
    setPayload(next);
    return next;
  }

  type EditorMode = "preview" | "source" | "css";
  const modes: { id: EditorMode; label: string; icon: typeof Presentation }[] = [
    { id: "preview", label: "预览", icon: Presentation },
    { id: "source", label: "Markdown 源码", icon: FileCode2 },
    { id: "css", label: "CSS 样式", icon: Braces },
  ];
  function selectMode(next: EditorMode) {
    setMode(next);
    if (next === "source" && source === null && !sourceLoading) void loadSource();
  }
  function onTabKeyDown(event: React.KeyboardEvent<HTMLDivElement>) {
    const index = modes.findIndex(item => item.id === mode);
    const next = event.key === "ArrowRight" ? (index + 1) % modes.length
      : event.key === "ArrowLeft" ? (index + modes.length - 1) % modes.length
      : event.key === "Home" ? 0 : event.key === "End" ? modes.length - 1 : -1;
    if (next < 0) return;
    event.preventDefault(); event.stopPropagation();
    selectMode(modes[next].id);
    event.currentTarget.querySelectorAll<HTMLButtonElement>('[role="tab"]')[next]?.focus();
  }

  const dark = useDeckDark();

  const onSlideSeenRef = useRef(onSlideSeen);
  onSlideSeenRef.current = onSlideSeen;
  useEffect(() => { onSlideSeenRef.current?.(currentSlide); }, [deckId, currentSlide]);

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
      <div className="deck-editor-toolbar">
        <div className="deck-mode-tabs" role="tablist" aria-label="演示文稿视图" onKeyDown={onTabKeyDown}>
          {modes.map(({ id, label, icon: Icon }) => <button
            key={id} type="button" role="tab" id={`${tabId}-tab-${id}`}
            aria-selected={mode === id} aria-controls={`${tabId}-panel-${id}`}
            tabIndex={mode === id ? 0 : -1} className="deck-mode-tab" onClick={() => selectMode(id)}
          ><Icon size={15} aria-hidden="true" /><span>{label}</span>{id === "source" && source !== null && source !== savedSource && <span className="deck-mode-dirty" aria-label="有未保存更改" />}</button>)}
        </div>
        {mode === "source" && <div className="deck-editor-actions">
          {source !== null && source !== savedSource && <span className="deck-save-status">未保存</span>}
          <button type="button" className="deck-save-btn" disabled={saving || sourceLoading || source === null || source === savedSource} onClick={() => void saveSource()}><Save size={14} aria-hidden="true" />{saving ? "保存中…" : "保存"}</button>
        </div>}
      </div>
      <div className="deck-preview-panel" role="tabpanel" id={`${tabId}-panel-preview`} aria-labelledby={`${tabId}-tab-preview`} hidden={mode !== "preview"}>{mode === "preview" && <DeckRenderer payload={payload} dark={dark} startSlide={currentSlide} onSlideChange={setCurrentSlide} />}</div>
      <div className="deck-source-host" role="tabpanel" id={`${tabId}-panel-source`} aria-labelledby={`${tabId}-tab-source`} hidden={mode !== "source"}><div className="deck-source">
        {sourceError && <div role="alert">{sourceError}</div>}
        {sourceLoading ? <p>加载源码…</p> : source === null ? <button type="button" className="deck-btn" onClick={() => void loadSource()}>重试</button> : <TextCodeEditor documentKey={`${deckId}/source.md`} language="markdown" label="Markdown 源码" value={source} readOnly={saving} onChange={setSource} onSave={() => { if (!saving && source !== savedSource) void saveSource(); }} />}
      </div></div>
      <div className="deck-css-editor-host" role="tabpanel" id={`${tabId}-panel-css`} aria-labelledby={`${tabId}-tab-css`} hidden={mode !== "css"}><DeckCssEditor payload={payload} currentSlide={currentSlide} refresh={refreshPayload} /></div>
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
