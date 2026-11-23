import { useEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
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
  const { t } = useTranslation("bibliography");
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
      if (!Array.isArray(response.models)) throw new Error(t("semantic.modelDiscoveryUnavailable"));
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
      if (draft.enabled && !/^[0-9a-f]{64}$/.test(draft.model_digest)) throw new Error(t("semantic.modelDigestRequired"));
      if (remote && !draft.allow_remote) throw new Error(t("semantic.allowRemoteRequired"));
      const response = await gatewayRequest("PUT", "/bib/search/semantic", draft);
      if (!validStatus(response.status)) throw new Error(t("semantic.statusMissing"));
      onSaved(response.status); onClose();
    } catch (failure) { setError(String(failure)); } finally { setBusy(false); }
  };
  const evidenceScope = (field: "include_fulltext" | "include_annotations", enabled: boolean) => profile({ [field]: enabled, ...(enabled && !draft.profile.chunk_chars ? { chunk_chars: 2000, overlap_chars: 200 } : {}) });
  return <SearchDialog title={t("semantic.settingsTitle")} onClose={onClose} footer={<div className="space-y-2 text-xs">
    {error && <p role="alert" className="text-destructive">{error}</p>}
    <div className="flex justify-end gap-2"><Button size="sm" variant="ghost" onClick={onClose}>{t("semantic.cancel")}</Button><Button size="sm" disabled={busy || modelsBusy} onClick={() => void save()}>{busy ? t("semantic.saving") : t("semantic.saveSettings")}</Button></div>
  </div>}>
    <div className="space-y-3 text-xs">
      <p>{t("semantic.intro")}</p>
      <p className="rounded bg-muted p-2">{t("semantic.indexingScope", { fulltext: draft.profile.include_fulltext ? t("semantic.scopeFulltext") : "", annotations: draft.profile.include_annotations ? t("semantic.scopeAnnotations") : "" })} {remote ? t("semantic.remoteDestination", { origin: draft.origin }) : t("semantic.localDestination")} {t("semantic.onlyInstalled")}</p>
      <label className="flex items-center gap-2"><input type="checkbox" checked={draft.enabled} onChange={event => setDraft(current => ({ ...current, enabled: event.target.checked }))} />{t("semantic.enableIndexing")}</label>
      <label className="block">{t("semantic.modelServer")}<Input aria-label={t("semantic.modelServerAria")} value={draft.origin} onChange={event => { modelSequence.current++; modelAbort.current?.abort(); setModelsBusy(false); setModels([]); setDraft(current => ({ ...current, origin: event.target.value, allow_remote: false })); }} /></label>
      {remote && <label className="flex items-start gap-2"><input type="checkbox" checked={draft.allow_remote} onChange={event => setDraft(current => ({ ...current, allow_remote: event.target.checked }))} />{t("semantic.allowRemote")}</label>}
      <div className="flex flex-wrap items-center gap-2"><Button size="xs" variant="secondary" disabled={modelsBusy || (remote && !draft.allow_remote)} onClick={() => void loadModels()}>{modelsBusy ? t("semantic.readingModels") : t("semantic.loadModels")}</Button><span className="text-muted-foreground">{t("semantic.chooseEmbeddingModel")}</span></div>
      {models.length > 0 && <label className="block">{t("semantic.installedModel")}<select aria-label={t("semantic.installedModelAria")} className="mt-1 w-full rounded border bg-background p-1" value={models.some(model => model.name === draft.model && model.digest === draft.model_digest) ? draft.model : ""} onChange={event => { const model = models.find(model => model.name === event.target.value); if (model) setDraft(current => ({ ...current, model: model.name, model_digest: model.digest })); }}><option value="">{t("semantic.chooseInstalledModel")}</option>{models.map(model => <option key={model.name} value={model.name}>{model.name}</option>)}</select></label>}
      {modelError && <p role="alert" className="text-destructive">{modelError}</p>}
      <p>{t("semantic.selectedModelPrefix")} <strong className="break-all">{draft.model}</strong>{t("semantic.selectedModelSuffix")}</p>
      <details><summary className="cursor-pointer">{t("semantic.advancedOptions")}</summary><div className="mt-2 space-y-2">
        <label className="block">{t("semantic.modelName")}<Input aria-label={t("semantic.modelNameAria")} value={draft.model} onChange={event => setDraft(current => ({ ...current, model: event.target.value, model_digest: "" }))} /></label>
        <label className="block">{t("semantic.manifestDigest")}<Input aria-label={t("semantic.manifestDigestAria")} value={draft.model_digest} onChange={event => setDraft(current => ({ ...current, model_digest: event.target.value }))} /></label>
        <label className="block">{t("semantic.vectorDimensions")}<Input aria-label={t("semantic.vectorDimensionsAria")} type="number" min="1" max="65536" value={draft.profile.dimensions} onChange={event => profile({ dimensions: Number(event.target.value) })} /></label>
        <p className="text-muted-foreground">{t("semantic.dimensionsHint")}</p>
        <label className="block">{t("semantic.contextWindow")}<Input aria-label={t("semantic.contextWindowAria")} type="number" min="1" max="131072" placeholder={t("semantic.serverDefault")} value={draft.inference?.num_ctx ?? ""} onChange={event => setDraft(current => ({ ...current, inference: { ...current.inference, num_ctx: event.target.value === "" ? null : Number(event.target.value) } }))} /></label>
        <label className="block">{t("semantic.tokenBatch")}<Input aria-label={t("semantic.tokenBatchAria")} type="number" min="1" max="8192" placeholder={t("semantic.serverDefault")} value={draft.inference?.num_batch ?? ""} onChange={event => setDraft(current => ({ ...current, inference: { ...current.inference, num_batch: event.target.value === "" ? null : Number(event.target.value) } }))} /></label>
        <p className="text-muted-foreground">{t("semantic.tokenLimitsHint")}</p>
        <label className="block">{t("semantic.queryInstruction")}<textarea aria-label={t("semantic.queryInstructionAria")} className="mt-1 min-h-16 w-full rounded border bg-background p-2" value={draft.profile.query_instruction} onChange={event => profile({ query_instruction: event.target.value })} /></label>
        <label className="flex items-center gap-2"><input type="checkbox" checked={draft.profile.include_annotations} onChange={event => evidenceScope("include_annotations", event.target.checked)} />{t("semantic.includeAnnotations")}</label>
        <label className="flex items-center gap-2"><input type="checkbox" checked={draft.profile.include_fulltext} onChange={event => evidenceScope("include_fulltext", event.target.checked)} />{t("semantic.includeFulltext")}</label>
        <label className="block">{t("semantic.windowSize")}<Input aria-label={t("semantic.windowSizeAria")} type="number" min="0" max="16384" value={draft.profile.chunk_chars ?? 0} onChange={event => profile({ chunk_chars: Number(event.target.value) || null, ...(Number(event.target.value) ? {} : { overlap_chars: 0 }) })} /></label>
        <label className="block">{t("semantic.windowOverlap")}<Input aria-label={t("semantic.windowOverlapAria")} type="number" min="0" value={draft.profile.overlap_chars} onChange={event => profile({ overlap_chars: Number(event.target.value) })} /></label>
      </div></details>
      <details><summary className="cursor-pointer">{t("semantic.resourceLimits")}</summary><div className="mt-2 space-y-2">
        <label className="block">{t("semantic.maxChunks")}<Input aria-label={t("semantic.maxChunksAria")} type="number" min="1" max="10000000" value={limits.max_chunks} onChange={event => limit("max_chunks", Number(event.target.value))} /></label>
        <label className="block">{t("semantic.maxInput")}<Input aria-label={t("semantic.maxInputAria")} type="number" min="1" max="16384" value={limits.max_input_bytes / 1024} onChange={event => limit("max_input_bytes", Number(event.target.value) * 1024)} /></label>
        <label className="block">{t("semantic.vectorBudget")}<Input aria-label={t("semantic.vectorBudgetAria")} type="number" min="1" max="1048576" value={limits.max_vector_bytes / 1024 ** 2} onChange={event => limit("max_vector_bytes", Number(event.target.value) * 1024 ** 2)} /></label>
        <label className="block">{t("semantic.diskBudget")}<Input aria-label={t("semantic.diskBudgetAria")} type="number" min="1" max="1048576" value={limits.max_disk_bytes / 1024 ** 2} onChange={event => limit("max_disk_bytes", Number(event.target.value) * 1024 ** 2)} /></label>
        <p className="text-muted-foreground">{t("semantic.budgetsHint")}</p>
      </div></details>
    </div>
  </SearchDialog>;
}

export function SemanticSearchControls({ onReady, onInvalidate, onKeyword }: { onReady: () => void; onInvalidate: () => void; onKeyword: () => void }) {
  const { t } = useTranslation("bibliography");
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
      if (!validStatus(next)) throw new Error(t("semantic.unavailable"));
      if (id === sequence.current) accept(next);
    } catch (failure) { if (id === sequence.current && !abort.current?.signal.aborted) setError(String(failure)); }
  };
  useEffect(() => { void refresh(); const timer = window.setInterval(() => void refresh(), 2000); return () => { sequence.current++; abort.current?.abort(); window.clearInterval(timer); }; }, []);
  const maintain = async (action: SemanticMaintenanceAction) => {
    if (!status) return;
    sequence.current++; abort.current?.abort();setMaintenanceBusy(true);setError("");
    try {
      const response = await gatewayRequest("POST", "/bib/search/semantic/maintenance", { action, expected_configuration_generation: status.configuration_generation, request_id: newGatewayRequestId() });
      if (!validStatus(response.status)) throw new Error(t("semantic.maintenanceStatusMissing"));
      sequence.current++;abort.current?.abort();accept(response.status);
    } catch (failure) { setError(String(failure)); } finally { setMaintenanceBusy(false); }
  };
  const labels: Record<SemanticStatus["phase"], string> = { disabled: t("semantic.phaseDisabled"), loading: t("semantic.phaseLoading"), indexing: t("semantic.phaseIndexing"), ready: t("semantic.phaseReady"), failed: t("semantic.phaseFailed"), paused: t("semantic.phasePaused") };
  const progress = status?.progress ?? status?.maintenance?.progress;
  return <section aria-label={t("semantic.statusSectionAria")} className="space-y-1 border-b px-2 py-2 text-xs">
    <div className="flex flex-wrap items-center gap-2"><span role="status">{status ? labels[status.phase] : t("semantic.checking")}</span><Button size="xs" variant="secondary" disabled={!status} onClick={() => setSettings(true)}>{t("semantic.topicSettings")}</Button><Button size="xs" variant="ghost" onClick={onKeyword}>{t("semantic.useKeywordSearch")}</Button></div>
    {status && <p className="break-words text-muted-foreground">{status.config.model} · {localOrigin(status.config.origin) ? t("semantic.localServerLabel") : t("semantic.remoteServerLabel", { origin: status.config.origin })} · {t("semantic.indexedChunks", { count: status.indexed_chunks.toLocaleString() })} · {status.config.profile.include_fulltext ? t("semantic.corpusFulltext") : t("semantic.corpusTitles")}{status.config.profile.include_annotations ? t("semantic.corpusAnnotationsSuffix") : ""}. {t("semantic.coverageNote")}</p>}
    {status?.resources && <p aria-label={t("semantic.cacheUsageAria")} className="text-muted-foreground">{t("semantic.lastObservedCache", { vectors: cacheBytes(status.resources.vector_bytes) })}{status.resources.disk_bytes !== null ? t("semantic.cacheDiskSuffix", { disk: cacheBytes(status.resources.disk_bytes) }) : ""}.</p>}
    {status?.config.enabled && <>
      <div className="flex flex-wrap items-center gap-2">
        <Button size="xs" variant="ghost" disabled={maintenanceBusy} onClick={() => void maintain("rebuild")}>{t("semantic.rebuildIndex")}</Button>
        <Button size="xs" variant="ghost" disabled={maintenanceBusy} onClick={() => void maintain("repair")}>{t("semantic.repairIndex")}</Button>
        <Button size="xs" variant="ghost" disabled={maintenanceBusy} onClick={() => void maintain(status.phase === "paused" ? "resume" : "cancel")}>{status.phase === "paused" ? t("semantic.resumeIndexing") : t("semantic.pauseIndexing")}</Button>
      </div>
      <p className="text-muted-foreground">{t("semantic.maintenanceHint")}</p>
    </>}
    {status && (status.phase === "loading" || status.phase === "indexing") && progress && <div aria-label={t("semantic.progressAria")} className="space-y-1">
      <p>{t(`semantic.stage_${progress.stage}`, { defaultValue: t("semantic.phaseIndexing") })}</p>
      <p>{t("semantic.progressStats", { processed: progress.processed_articles.toLocaleString(), target: progress.target_articles?.toLocaleString() ?? t("semantic.unknownTarget"), chunks: progress.processed_chunks.toLocaleString(), newVectors: progress.embedded_chunks.toLocaleString(), cached: progress.cached_chunks.toLocaleString() })}</p>
      {Boolean(progress.target_articles) && <progress aria-label={t("semantic.progressArticleAria")} className="h-2 w-full" max={progress.target_articles!} value={Math.min(progress.processed_articles,progress.target_articles!)} />}
    </div>}
    {status?.maintenance && <p aria-label={t("semantic.maintenanceAria")} className="text-muted-foreground">{t("semantic.maintenanceState", { action: status.maintenance.action, state: status.maintenance.state })}</p>}
    {status?.last_error && <p role={status.phase === "ready" ? "status" : "alert"} className={status.phase === "ready" ? "text-amber-700 dark:text-amber-400" : "text-destructive"}>{status.phase === "ready" ? t("semantic.searchReadyPrefix") : ""}{status.last_error}</p>}
    {error && <p role="alert" className="text-destructive">{error}<Button size="xs" variant="ghost" onClick={() => void refresh()}>{t("semantic.retryStatus")}</Button></p>}
    {settings && status && <SemanticSettings config={status.config} onClose={() => setSettings(false)} onSaved={next => { sequence.current++; abort.current?.abort(); accept(next); }} />}
  </section>;
}

export function RecallLabel({ recall }: { recall?: SearchRecall }) {
  const { t } = useTranslation("bibliography");
  if (!recall) return null;
  return <span className="mr-2 font-medium" title={t("semantic.recallTitle", { lexical: recall.lexical_rank ?? t("semantic.notRecalled"), semantic: recall.semantic_rank ?? t("semantic.notRecalled") })}>{recall.lexical_rank && recall.semantic_rank ? t("semantic.keywordPlusSemantic") : recall.semantic_rank ? t("semantic.semanticSimilarity") : t("semantic.keywordMatch")}</span>;
}
