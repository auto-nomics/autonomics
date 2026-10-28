import { useEffect, useRef, useState } from "react";
import { gatewayRequest, newGatewayRequestId } from "@/gateway";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { SearchDialog } from "./search-dialog";
import type { SearchRecall, SemanticConfig, SemanticStatus, SemanticMaintenanceAction, SemanticLimits } from "./bib-search-model";

const defaultLimits: SemanticLimits = { max_chunks: 1_000_000, max_input_bytes: 262_144, max_vector_bytes: 2 * 1024 ** 3, max_disk_bytes: 8 * 1024 ** 3 };
function cacheBytes(bytes: number) { return bytes < 1024 ? `${bytes} B` : bytes < 1024 ** 2 ? `${(bytes / 1024).toFixed(1)} KiB` : `${(bytes / 1024 ** 2).toFixed(1)} MiB`; }

function localOrigin(origin: string) {
  try { const host = new URL(origin).hostname; return host === "localhost" || host === "[::1]" || /^127\.\d+\.\d+\.\d+$/.test(host); }
  catch { return false; }
}
function validStatus(value: any): value is SemanticStatus {
  return value && ["disabled", "loading", "indexing", "ready", "failed", "paused"].includes(value.phase) && typeof value.config?.enabled === "boolean" && value.config?.profile && typeof value.configuration_generation === "string";
}

function SemanticSettings({ config, onSaved, onClose }: { config: SemanticConfig; onSaved: (status: SemanticStatus) => void; onClose: () => void }) {
  const [draft, setDraft] = useState<SemanticConfig>(() => ({ ...config, profile: { ...config.profile } }));
  const [models, setModels] = useState<Array<{ name: string; digest: string }>>([]);
  const [modelError, setModelError] = useState("");
  const [modelsBusy, setModelsBusy] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  const modelSequence = useRef(0);
  const modelAbort = useRef<AbortController | null>(null);
  const remote = !localOrigin(draft.origin);
  const profile = (patch: Partial<SemanticConfig["profile"]>) => setDraft(current => ({ ...current, profile: { ...current.profile, ...patch } }));
  const limits = { ...defaultLimits, ...draft.limits };
  const limit = (key: keyof SemanticLimits, value: number) => setDraft(current => ({ ...current, limits: { ...defaultLimits, ...current.limits, [key]: Math.round(value) } }));
  const loadModels = async () => {
    const sequence = ++modelSequence.current;
    modelAbort.current?.abort(); modelAbort.current = new AbortController();
    setModelsBusy(true); setModelError("");
    try {
      const response = await gatewayRequest("POST", "/bib/search/semantic/models", { origin: draft.origin, allow_remote: draft.allow_remote }, { signal: modelAbort.current.signal });
      if (!Array.isArray(response.models)) throw new Error("Installed model discovery is unavailable on this server.");
      if (sequence !== modelSequence.current) return;
      const installed = response.models.filter((model: any) => typeof model.name === "string" && typeof model.digest === "string" && !model.name.startsWith("autonomics-embedding:"));
      setModels(installed);
      setDraft(current => {
        const match = installed.find((model: { name: string }) => model.name === current.model || model.name === `${current.model}:latest`);
        return !current.model_digest && match ? { ...current, model: match.name, model_digest: match.digest } : current;
      });
    } catch (failure) { if (sequence === modelSequence.current && !modelAbort.current?.signal.aborted) setModelError(String(failure)); }
    finally { if (sequence === modelSequence.current) setModelsBusy(false); }
  };
  useEffect(() => { if (localOrigin(config.origin) && !config.model_digest) void loadModels(); return () => { modelSequence.current++; modelAbort.current?.abort(); }; }, []);
  const save = async () => {
    setBusy(true); setError("");
    try {
      if (draft.enabled && !/^[0-9a-f]{64}$/.test(draft.model_digest)) throw new Error("Choose an installed embedding model or provide its manifest digest.");
      if (remote && !draft.allow_remote) throw new Error("Allow remote text processing explicitly before using this server.");
      const response = await gatewayRequest("PUT", "/bib/search/semantic", draft);
      if (!validStatus(response.status)) throw new Error("The server did not return a semantic indexing status. Refresh before retrying.");
      onSaved(response.status); onClose();
    } catch (failure) { setError(String(failure)); } finally { setBusy(false); }
  };
  const evidenceScope = (field: "include_fulltext" | "include_annotations", enabled: boolean) => profile({ [field]: enabled, ...(enabled && !draft.profile.chunk_chars ? { chunk_chars: 2000, overlap_chars: 200 } : {}) });
  return <SearchDialog title="Topic search settings" onClose={onClose} footer={<div className="space-y-2 text-xs">
    {error && <p role="alert" className="text-destructive">{error}</p>}
    <div className="flex justify-end gap-2"><Button size="sm" variant="ghost" onClick={onClose}>Cancel</Button><Button size="sm" disabled={busy || modelsBusy} onClick={() => void save()}>{busy ? "Saving…" : "Save topic settings"}</Button></div>
  </div>}>
    <div className="space-y-3 text-xs">
      <p>Topic search combines keyword matches with a model's similarity ranking. It is experimental; use keyword search for precise lookup.</p>
      <p className="rounded bg-muted p-2">Indexing processes titles and abstracts across this bibliography database{draft.profile.include_fulltext ? ", extracted attachment text" : ""}{draft.profile.include_annotations ? ", and annotations" : ""}. {remote ? `Selected text will be sent to ${draft.origin}.` : "The configured model server runs on the bibliography host."} Only installed models are used; saving does not download a model.</p>
      <label className="flex items-center gap-2"><input type="checkbox" checked={draft.enabled} onChange={event => setDraft(current => ({ ...current, enabled: event.target.checked }))} />Enable indexing of this selected library text</label>
      <label className="block">Model server<Input aria-label="Semantic model server" value={draft.origin} onChange={event => { modelSequence.current++; modelAbort.current?.abort(); setModelsBusy(false); setModels([]); setDraft(current => ({ ...current, origin: event.target.value, allow_remote: false })); }} /></label>
      {remote && <label className="flex items-start gap-2"><input type="checkbox" checked={draft.allow_remote} onChange={event => setDraft(current => ({ ...current, allow_remote: event.target.checked }))} />Allow sending the selected library text to this remote server</label>}
      <div className="flex flex-wrap items-center gap-2"><Button size="xs" variant="secondary" disabled={modelsBusy || (remote && !draft.allow_remote)} onClick={() => void loadModels()}>{modelsBusy ? "Reading models…" : "Load installed models"}</Button><span className="text-muted-foreground">Choose a model that supports embeddings.</span></div>
      {models.length > 0 && <label className="block">Installed model<select aria-label="Installed embedding model" className="mt-1 w-full rounded border bg-background p-1" value={models.some(model => model.name === draft.model && model.digest === draft.model_digest) ? draft.model : ""} onChange={event => { const model = models.find(model => model.name === event.target.value); if (model) setDraft(current => ({ ...current, model: model.name, model_digest: model.digest })); }}><option value="">Choose an installed embedding model</option>{models.map(model => <option key={model.name} value={model.name}>{model.name}</option>)}</select></label>}
      {modelError && <p role="alert" className="text-destructive">{modelError}</p>}
      <p>Selected model: <strong className="break-all">{draft.model}</strong>. Changing its version builds a separate index.</p>
      <details><summary className="cursor-pointer">Advanced model and content options</summary><div className="mt-2 space-y-2">
        <label className="block">Installed model name<Input aria-label="Semantic model name" value={draft.model} onChange={event => setDraft(current => ({ ...current, model: event.target.value, model_digest: "" }))} /></label>
        <label className="block">Manifest digest<Input aria-label="Semantic model digest" value={draft.model_digest} onChange={event => setDraft(current => ({ ...current, model_digest: event.target.value }))} /></label>
        <label className="block">Vector dimensions<Input aria-label="Semantic vector dimensions" type="number" min="1" max="65536" value={draft.profile.dimensions} onChange={event => profile({ dimensions: Number(event.target.value) })} /></label>
        <p className="text-muted-foreground">The evaluated Qwen3-Embedding 0.6B model uses 1,024 dimensions. Other models need their documented dimensions and instruction.</p>
        <label className="block">Context window in tokens<Input aria-label="Semantic context tokens" type="number" min="1" max="131072" placeholder="Model server default" value={draft.inference?.num_ctx ?? ""} onChange={event => setDraft(current => ({ ...current, inference: { ...current.inference, num_ctx: event.target.value === "" ? null : Number(event.target.value) } }))} /></label>
        <label className="block">Token batch size<Input aria-label="Semantic token batch" type="number" min="1" max="8192" placeholder="Model server default" value={draft.inference?.num_batch ?? ""} onChange={event => setDraft(current => ({ ...current, inference: { ...current.inference, num_batch: event.target.value === "" ? null : Number(event.target.value) } }))} /></label>
        <p className="text-muted-foreground">Leave blank to use server defaults. These are model token limits, separate from the number of documents per request. Changing an override builds a separate vector cache; use values supported by the installed model.</p>
        <label className="block">Query instruction<textarea aria-label="Semantic query instruction" className="mt-1 min-h-16 w-full rounded border bg-background p-2" value={draft.profile.query_instruction} onChange={event => profile({ query_instruction: event.target.value })} /></label>
        <label className="flex items-center gap-2"><input type="checkbox" checked={draft.profile.include_annotations} onChange={event => evidenceScope("include_annotations", event.target.checked)} />Include annotations</label>
        <label className="flex items-center gap-2"><input type="checkbox" checked={draft.profile.include_fulltext} onChange={event => evidenceScope("include_fulltext", event.target.checked)} />Include extracted attachment text</label>
        <label className="block">Window size in characters (0 = complete abstract)<Input aria-label="Semantic window characters" type="number" min="0" max="16384" value={draft.profile.chunk_chars ?? 0} onChange={event => profile({ chunk_chars: Number(event.target.value) || null, ...(Number(event.target.value) ? {} : { overlap_chars: 0 }) })} /></label>
        <label className="block">Window overlap in characters<Input aria-label="Semantic window overlap" type="number" min="0" value={draft.profile.overlap_chars} onChange={event => profile({ overlap_chars: Number(event.target.value) })} /></label>
      </div></details>
      <details><summary className="cursor-pointer">Index resource limits</summary><div className="mt-2 space-y-2">
        <label className="block">Maximum indexed text chunks<Input aria-label="Semantic maximum chunks" type="number" min="1" max="10000000" value={limits.max_chunks} onChange={event => limit("max_chunks", Number(event.target.value))} /></label>
        <label className="block">Maximum model input (KiB)<Input aria-label="Semantic maximum input KiB" type="number" min="1" max="16384" value={limits.max_input_bytes / 1024} onChange={event => limit("max_input_bytes", Number(event.target.value) * 1024)} /></label>
        <label className="block">Vector payload budget per store (MiB)<Input aria-label="Semantic vector budget MiB" type="number" min="1" max="1048576" value={limits.max_vector_bytes / 1024 ** 2} onChange={event => limit("max_vector_bytes", Number(event.target.value) * 1024 ** 2)} /></label>
        <label className="block">All topic cache files budget (MiB)<Input aria-label="Semantic disk budget MiB" type="number" min="1" max="1048576" value={limits.max_disk_bytes / 1024 ** 2} onChange={event => limit("max_disk_bytes", Number(event.target.value) * 1024 ** 2)} /></label>
        <p className="text-muted-foreground">Active, previous-model and unfinished cache data count toward these budgets. The file budget stops indexing between batches; temporary writes can exceed it. Changing limits preserves valid vectors and does not compact existing files.</p>
      </div></details>
    </div>
  </SearchDialog>;
}

export function SemanticSearchControls({ onReady, onInvalidate, onKeyword }: { onReady: () => void; onInvalidate: () => void; onKeyword: () => void }) {
  const [status, setStatus] = useState<SemanticStatus | null>(null);
  const [error, setError] = useState("");
  const [settings, setSettings] = useState(false);
  const [maintenanceBusy, setMaintenanceBusy] = useState(false);
  const sequence = useRef(0);
  const abort = useRef<AbortController | null>(null);
  const previous = useRef<SemanticStatus | null>(null);
  const callbacks = useRef({ onReady, onInvalidate }); callbacks.current = { onReady, onInvalidate };
  const accept = (next: SemanticStatus) => {
    const old = previous.current;
    previous.current = next; setStatus(next); setError("");
    if (old && old.configuration_generation !== next.configuration_generation) callbacks.current.onInvalidate();
    if (old && next.phase === "ready" && (old.phase !== "ready" || old.generation !== next.generation)) callbacks.current.onReady();
  };
  const refresh = async () => {
    const id = ++sequence.current; abort.current?.abort(); abort.current = new AbortController();
    try {
      const next = await gatewayRequest("GET", "/bib/search/semantic", undefined, { signal: abort.current.signal });
      if (!validStatus(next)) throw new Error("Topic search is unavailable on this server.");
      if (id === sequence.current) accept(next);
    } catch (failure) { if (id === sequence.current && !abort.current?.signal.aborted) setError(String(failure)); }
  };
  useEffect(() => { void refresh(); const timer = window.setInterval(() => void refresh(), 2000); return () => { sequence.current++; abort.current?.abort(); window.clearInterval(timer); }; }, []);
  const maintain = async (action: SemanticMaintenanceAction) => {
    if (!status) return;
    sequence.current++; abort.current?.abort();setMaintenanceBusy(true);setError("");
    try {
      const response = await gatewayRequest("POST", "/bib/search/semantic/maintenance", { action, expected_configuration_generation: status.configuration_generation, request_id: newGatewayRequestId() });
      if (!validStatus(response.status)) throw new Error("The server did not return a maintenance status. Refresh before retrying.");
      sequence.current++;abort.current?.abort();accept(response.status);
    } catch (failure) { setError(String(failure)); } finally { setMaintenanceBusy(false); }
  };
  const labels = { disabled: "Topic indexing is off", loading: "Connecting to model…", indexing: "Updating topic index…", ready: "Topic index ready", failed: "Topic indexing needs attention", paused: "Topic indexing paused" };
  const progress = status?.progress ?? status?.maintenance?.progress;
  return <section aria-label="Topic search status" className="space-y-1 border-b px-2 py-2 text-xs">
    <div className="flex flex-wrap items-center gap-2"><span role="status">{status ? labels[status.phase] : "Checking topic index…"}</span><Button size="xs" variant="secondary" disabled={!status} onClick={() => setSettings(true)}>Topic settings</Button><Button size="xs" variant="ghost" onClick={onKeyword}>Use keyword search</Button></div>
    {status && <p className="break-words text-muted-foreground">{status.config.model} · {localOrigin(status.config.origin) ? "model server on the bibliography host" : `remote model server: ${status.config.origin}`} · {status.indexed_chunks.toLocaleString()} indexed text chunks · {status.config.profile.include_fulltext ? "abstracts and extracted text" : "titles and abstracts"}{status.config.profile.include_annotations ? " and annotations" : ""}. Index coverage is not the number of matching articles.</p>}
    {status?.resources && <p aria-label="Topic cache usage" className="text-muted-foreground">Last observed cache: {cacheBytes(status.resources.vector_bytes)} vector payload in this store{status.resources.disk_bytes !== null ? ` · ${cacheBytes(status.resources.disk_bytes)} across topic cache files` : ""}.</p>}
    {status?.config.enabled && <>
      <div className="flex flex-wrap items-center gap-2">
        <Button size="xs" variant="ghost" disabled={maintenanceBusy} onClick={() => void maintain("rebuild")}>Rebuild topic index</Button>
        <Button size="xs" variant="ghost" disabled={maintenanceBusy} onClick={() => void maintain("repair")}>Repair topic index</Button>
        <Button size="xs" variant="ghost" disabled={maintenanceBusy} onClick={() => void maintain(status.phase === "paused" ? "resume" : "cancel")}>{status.phase === "paused" ? "Resume indexing" : "Pause indexing"}</Button>
      </div>
      <p className="text-muted-foreground">Rebuild reuses valid vectors. Repair regenerates selected text in a separate index and switches only when complete. Pausing retains completed batches for resume.</p>
    </>}
    {status && (status.phase === "loading" || status.phase === "indexing") && progress && <div aria-label="Topic index progress" className="space-y-1">
      <p>{({ planning: "Checking changes", staging: "Preparing index update", copying_bindings: "Reusing indexed text", embedding: "Indexing changed text", publishing: "Publishing updated index", complete: "Index up to date" } as Record<string,string>)[progress.stage] ?? "Updating topic index"}</p>
      <p>{progress.processed_articles.toLocaleString()} / {progress.target_articles?.toLocaleString() ?? "unknown"} articles processed · {progress.processed_chunks.toLocaleString()} text chunks committed · {progress.embedded_chunks.toLocaleString()} new vectors · {progress.cached_chunks.toLocaleString()} cached chunks</p>
      {Boolean(progress.target_articles) && <progress aria-label="Topic indexing article progress" className="h-2 w-full" max={progress.target_articles!} value={Math.min(progress.processed_articles,progress.target_articles!)} />}
    </div>}
    {status?.maintenance && <p aria-label="Topic maintenance state" className="text-muted-foreground">Maintenance: {status.maintenance.action} · {status.maintenance.state}</p>}
    {status?.last_error && <p role={status.phase === "ready" ? "status" : "alert"} className={status.phase === "ready" ? "text-amber-700 dark:text-amber-400" : "text-destructive"}>{status.phase === "ready" ? "Search is ready. " : ""}{status.last_error}</p>}
    {error && <p role="alert" className="text-destructive">{error}<Button size="xs" variant="ghost" onClick={() => void refresh()}>Retry topic status</Button></p>}
    {settings && status && <SemanticSettings config={status.config} onClose={() => setSettings(false)} onSaved={next => { sequence.current++; abort.current?.abort(); accept(next); }} />}
  </section>;
}

export function RecallLabel({ recall }: { recall?: SearchRecall }) {
  if (!recall) return null;
  return <span className="mr-2 font-medium" title={`Keyword rank: ${recall.lexical_rank ?? "not recalled"}; semantic rank: ${recall.semantic_rank ?? "not recalled"}`}>{recall.lexical_rank && recall.semantic_rank ? "Keyword + semantic" : recall.semantic_rank ? "Semantic similarity" : "Keyword match"}</span>;
}
