import "./styles.css";
import {
  gatewayRequest as request,
  gatewayRequestOptional as requestOptional,
  probeGateway,
  startGatewayEvents,
  reportFrontendReady,
  pickUploadFile,
  uploadBibFulltext,
  saveTextFile,
} from "./gateway";
import { escapeHtml, renderMarkdown } from "./markdown";
import { createAppState, type View } from "./state";
import { openPdfReader } from "./pdf-reader";
import type {
  BibAnnotation,
  BibArticle,
  BibRequest,
  ChatItem,
  AgentRuntimeConfigView,
  AgentRuntimeOverrides,
  DagSnapshot,
  GatewayFrame,
  KmsProposal,
  KmsRow,
  SessionInfo,
  StoredAgent,
  BackendMessage,
  SkillLibraryRow,
  StateSnapshot,
  CustomModelDraft,
} from "./types";

const state = createAppState();

const app = document.querySelector<HTMLDivElement>("#app")!;
let renderScheduled = false;
let lastFrontendReadyStage = "";

function scheduleRender() {
  if (renderScheduled) return;
  renderScheduled = true;
  requestAnimationFrame(() => {
    renderScheduled = false;
    if (state.connected) render();
  });
}

function showRuntimeError(message: string) {
  let overlay = document.querySelector<HTMLDivElement>("#runtime-error");
  if (!overlay) {
    overlay = document.createElement("div");
    overlay.id = "runtime-error";
    document.body.append(overlay);
  }
  overlay.textContent = message;
}

window.addEventListener("error", (event) => showRuntimeError(`Error: ${event.message}`));
window.addEventListener("unhandledrejection", (event) => showRuntimeError(`Rejection: ${String(event.reason)}`));

async function loadBibliography(force = false) {
  if ((state.bibLoaded || state.bibLoading) && !force) return;
  state.bibLoading = true;
  try {
    const [collectionResponse, articleResponse] = await Promise.all([
      request("GET", "/bib/collections"),
      request(
        "GET",
        `/bib/articles?query=${encodeURIComponent(state.bibQuery)}&limit=500`,
      ),
    ]);
    state.bibCollections = collectionResponse.collections ?? [];
    state.bibArticles = articleResponse.articles ?? [];
    state.bibTotal = articleResponse.total ?? state.bibArticles.length;
    state.bibLoaded = true;
    if (state.bibSelectedCollection && !state.bibCollections.some((item) => item.id === state.bibSelectedCollection)) {
      state.bibSelectedCollection = null;
    }
    if (!bibArticles().some((article) => article.id === state.bibSelectedArticle)) {
      state.bibSelectedArticle = bibArticles()[0]?.id ?? null;
      state.bibDetail = null;
      void loadBibliographyDetail(state.bibSelectedArticle);
    }
  } catch (error) {
    state.bibError = String(error);
  } finally {
    state.bibLoading = false;
    render();
  }
}

async function loadBibliographyDetail(articleId: string | null) {
  if (!articleId) {
    state.bibDetail = null;
    return;
  }
  state.bibDetailLoading = true;
  render();
  try {
    state.bibDetail = await request(
      "GET",
      `/bib/articles/${encodeURIComponent(articleId)}`,
    );
  } catch (error) {
    state.bibDetail = null;
    state.bibError = String(error);
  } finally {
    state.bibDetailLoading = false;
    render();
  }
}

async function uploadBibliographyFulltext(articleId: string) {
  if (state.bibUploadBusy) return;
  const selected = await pickUploadFile();
  if (!selected) return;
  state.bibUploadBusy = true;
  state.bibError = null;
  render();
  try {
    await uploadBibFulltext(articleId, selected);
    await loadBibliography(true);
    await loadBibliographyDetail(articleId);
  } catch (error) {
    state.bibError = String(error);
  } finally {
    state.bibUploadBusy = false;
    render();
  }
}

async function exportBibliography() {
  if (state.bibExportBusy) return;
  const format = state.bibExportFormat;
  const extension = format === "csl_json" ? "json" : format;
  const collection = state.bibCollections.find(
    (item) => item.id === state.bibSelectedCollection,
  );
  const slug = (collection?.name ?? "all-articles")
    .toLowerCase()
    .replaceAll(/[^a-z0-9]+/g, "-")
    .replace(/^-+|-+$/g, "") || "all-articles";
  const query = new URLSearchParams({ format, limit: "500" });
  if (collection) query.set("collection_id", collection.id);
  state.bibExportBusy = true;
  state.bibError = null;
  render();
  try {
    const result = await request(
      "GET",
      `/bib/export?${query.toString()}`,
    );
    const path = await saveTextFile(
      `autonomics-${slug}.${extension}`,
      result.export ?? "",
    );
    if (!path) return;
    const size = (result.export ?? "").length;
    showCopyStatus(`Exported ${result.count ?? 0} articles (${size.toLocaleString()} bytes)`);
  } catch (error) {
    state.bibError = String(error);
  } finally {
    state.bibExportBusy = false;
    render();
  }
}

async function loadBibRequests(force = false) {
  if (state.bibRequestsLoading && !force) return;
  state.bibRequestsLoading = true;
  render();
  try {
    const response = await request("GET", "/bib/requests");
    state.bibRequests = response.requests ?? [];
  } catch (error) {
    state.bibError = String(error);
  } finally {
    state.bibRequestsLoading = false;
    render();
  }
}

function openBibRequests() {
  state.bibRequestsOpen = true;
  void loadBibRequests(true);
  render();
}

function closeBibRequests() {
  state.bibRequestsOpen = false;
  render();
}

function openBibRequest(request: BibRequest) {
  state.bibRequestsOpen = false;
  state.bibSelectedArticle = request.article_id;
  state.bibDetailTab = "fulltext";
  state.bibDetail = null;
  void loadBibliographyDetail(request.article_id);
  render();
}

function openBibImport() {
  state.bibImportOpen = true;
  render();
}

function closeBibImport() {
  state.bibImportOpen = false;
  state.bibImport = { idType: "doi", id: "", source: "", fetchFulltext: true };
  render();
}

async function importBibArticle() {
  const draft = state.bibImport;
  if (!draft.id.trim() || state.bibImportBusy) return;
  state.bibImportBusy = true;
  state.bibError = null;
  render();
  try {
    const result = await request("POST", "/bib/articles/import", {
      id_type: draft.idType,
      id: draft.id.trim(),
      source: draft.source.trim() || null,
      fetch_fulltext: draft.fetchFulltext,
    });
    const articleId = result.article?.id;
    state.bibImportOpen = false;
    state.bibImport = { idType: draft.idType, id: "", source: "", fetchFulltext: true };
    await loadBibliography(true);
    if (articleId) {
      state.bibSelectedArticle = articleId;
      state.bibDetailTab = "metadata";
      await loadBibliographyDetail(articleId);
    }
  } catch (error) {
    state.bibError = String(error);
  } finally {
    state.bibImportBusy = false;
    render();
  }
}

async function hydrate() {
  try {
    const probe = await probeGateway();
    if (!probe.running) {
      state.connected = false;
      renderOffline(`${probe.addr} (token ${probe.token_found ? "found" : "missing"})`);
      return;
    }
    const snapshot: StateSnapshot = await request("GET", "/state");
    state.lastSeq = snapshot.last_seq;
    state.agents = snapshot.agents;
    state.sessions = snapshot.sessions;
    for (const [agent, sessions] of Object.entries(snapshot.sessions)) {
      if (!state.activeSessions.get(agent) && sessions.length) {
        state.activeSessions.set(
          agent,
          [...sessions].sort((left, right) => right.last_active - left.last_active)[0].id,
        );
      }
    }
    state.display = snapshot.display_settings;
    state.catalog = snapshot.model_catalog;
  state.profiles = snapshot.profiles.map(String);
    state.activeModel = snapshot.active_model_spec;
  state.spawnProfile = state.spawnProfile || snapshot.profiles[0] || "";
    state.connected = true;
    if (!state.activeAgent || !state.agents.some((agent) => agent.path === state.activeAgent)) {
      state.activeAgent = state.agents[0]?.path ?? "";
    }
    render();
    await startGatewayEvents(state.lastSeq, {
      onFrame: handleFrame,
      onStatus: (status) => {
        state.connected = status !== "disconnected";
        if (status === "reconnected") void hydrate();
        render();
      },
    });
    await Promise.all(state.agents.filter((agent) => !state.sessions[agent.path]?.length).map(async (agent) => {
      try {
        await request("GET", `/agents/${agent.path}/sessions`);
      } catch {
        // Session lists arrive asynchronously through the SSE stream.
      }
    }));
    if (state.activeAgent) await loadAgentModel(state.activeAgent);
    await loadHistory(state.activeAgent, true);
    render();
  } catch (error) {
    state.connected = false;
    renderOffline(String(error));
  }
}

async function loadAgentModel(agent: string) {
  try {
    const model = await request("GET", `/agents/${agent}/model`);
    const info = state.agents.find((item) => item.path === agent);
    if (info) {
      info.model = model.model ?? info.model;
      info.context_length = model.context_length ?? info.context_length;
    }
  } catch {
    // A live agent may not expose model metadata until its first turn.
  }
}

function latestSessionId(agent: string): string | null {
  const sessions = state.sessions[agent] ?? [];
  const active = state.activeSessions.get(agent);
  if (active && sessions.some((session) => session.id === active)) return active;
  const latest = [...sessions].sort((left, right) => right.last_active - left.last_active)[0];
  return latest?.id ?? null;
}

function conversationKey(agent: string, sessionId: string | null): string {
  return `${agent}:${sessionId ?? "live"}`;
}

async function loadHistory(agent: string, force = false) {
  const info = state.agents.find((item) => item.path === agent);
  const sessionId = latestSessionId(agent);
  const key = conversationKey(agent, sessionId);
  if (state.activeAgent !== agent && !force) return;
  if (!info?.agent_id || !sessionId || (!force && state.historyLoaded.get(agent) === key)) return;
  try {
    const messages: any[] = await request(
      "GET",
      `/agents/${info.agent_id}/sessions/${sessionId}/history`,
    );
    if (state.activeAgent === agent || force) {
      state.rawMessages.set(key, messages as BackendMessage[]);
      state.transcripts.set(agent, messages.flatMap(convertMessage));
      state.streaming.delete(agent);
      state.historyLoaded.set(agent, key);
    }
    try {
      const plan = await request("GET", `/agents/${info.agent_id}/sessions/${sessionId}/plan`);
      if (plan) state.plans.set(agent, plan);
    } catch {
      // Plans are optional.
    }
  } catch (error) {
    if (state.activeAgent === agent) {
      appendItem(agent, "error", `加载历史失败: ${error}`);
    }
    return false;
  }
}

function convertMessage(message: any): ChatItem[] {
  const output: ChatItem[] = [];
  const usage = message.usage
    ? `输入 ${message.usage.input_tokens ?? 0} · 输出 ${message.usage.output_tokens ?? 0} · 缓存 ${message.usage.cache_read_input_tokens ?? 0}`
    : undefined;
  let text = "";
  const pushText = () => {
    if (!text) return;
    output.push(makeItem(message.role === "user" ? "user" : "assistant", text));
    text = "";
  };
  for (const block of message.content ?? []) {
    if (block.type === "text") {
      text += text ? "\n" : "";
      text += block.text;
    } else if (block.type === "thinking") {
      pushText();
      output.push(makeItem("thinking", block.thinking, "thinking"));
    } else if (block.type === "tool_use") {
      pushText();
      output.push(makeItem("tool", `${block.name}: ${JSON.stringify(block.input)}`, "tool_call"));
    } else if (block.type === "tool_result") {
      pushText();
      output.push(makeItem("tool", `${block.is_error ? "ERR" : "OK"} ${block.content ?? ""}`, "tool_result"));
    }
  }
  pushText();
  if (output.length && usage && output[output.length - 1].role === "assistant") {
    output[output.length - 1].usage = usage;
  }
  return output;
}

function makeItem(role: ChatItem["role"], text: string, kind?: ChatItem["kind"]): ChatItem {
  return { id: ++state.itemSeq, role, text, kind, time: Date.now() };
}

function transcriptFor(agent: string): ChatItem[] {
  let transcript = state.transcripts.get(agent);
  if (!transcript) {
    transcript = [];
    state.transcripts.set(agent, transcript);
  }
  return transcript;
}

function appendItem(
  agent: string,
  role: ChatItem["role"],
  text: string,
  kind?: ChatItem["kind"],
): ChatItem {
  const item = makeItem(role, text, kind);
  transcriptFor(agent).push(item);
  return item;
}

function eventTag(event: any): string {
  if (typeof event !== "object" || event === null) {
    return typeof event === "string" ? event : "";
  }
  const key = Object.keys(event)[0];
  if (key && ["TextDelta", "ThinkingDelta", "ToolCall"].includes(key)) {
    return key;
  }
  return key && event[key] && typeof event[key] === "object"
    ? `${key}:${Object.keys(event[key])[0] ?? ""}`
    : key ?? "";
}

function handleAgentFrame(agent: string, event: any) {
  const transcript = transcriptFor(agent);
  const tag = eventTag(event);
  const usage = state.usage.get(agent) ?? { input: 0, output: 0, cache: 0 };
  if (event.TextDelta !== undefined) {
    let current = state.streaming.get(agent);
    if (!current) {
      current = makeItem("assistant", "");
      state.streaming.set(agent, current);
      transcript.push(current);
    }
    current.text += event.TextDelta;
  } else if (event.ThinkingDelta !== undefined) {
    let current = state.streaming.get(agent);
    if (!current || current.role !== "thinking") {
      current = makeItem("thinking", "");
      state.streaming.set(agent, current);
      transcript.push(current);
    }
    current.text += event.ThinkingDelta;
  } else if (event.LlmResponse !== undefined) {
    // The incremental deltas already built the visible assistant item.
  } else if (event.Thinking !== undefined) {
    // The incremental deltas already built the visible thinking item.
  } else if (event.ToolCall !== undefined) {
    appendItem(agent, "tool", `${event.ToolCall.name}: ${JSON.stringify(event.ToolCall.input)}`, "tool_call");
  } else if (event.ToolResult !== undefined) {
    appendItem(agent, "tool", `${event.ToolResult.ok ? "OK" : "ERR"} ${event.ToolResult.content}`, "tool_result");
  } else if (event.ToolCallBackground !== undefined) {
    appendItem(agent, "tool", `background #${event.ToolCallBackground.seq}: ${event.ToolCallBackground.name}`);
  } else if (event.ToolBackgroundComplete !== undefined) {
    appendItem(agent, "tool", `background #${event.ToolBackgroundComplete.seq} ${event.ToolBackgroundComplete.ok ? "完成" : "失败"}`);
  } else if (event.UserMessageAcknowledged !== undefined) {
    appendItem(agent, "user", event.UserMessageAcknowledged);
    if (!promptHistory(agent).includes(event.UserMessageAcknowledged)) {
      pushPromptHistory(agent, event.UserMessageAcknowledged);
    }
  } else if (event.Error !== undefined) {
    appendItem(agent, "error", event.Error);
  } else if (event.RetryableError !== undefined) {
    appendItem(agent, "error", `${event.RetryableError.message} (${event.RetryableError.attempt}/${event.RetryableError.max_retries})`);
  } else if (event.TurnAborted !== undefined) {
    appendItem(agent, "status", "已取消");
  } else if (event.PlanUpdate !== undefined) {
    state.plans.set(agent, event.PlanUpdate);
  } else if (event.SessionActivated !== undefined) {
    state.activeSessions.set(agent, event.SessionActivated.id);
    if (event.SessionActivated.title) {
      const sessions = state.sessions[agent] ?? [];
      const session = sessions.find((item) => item.id === event.SessionActivated.id);
      if (session) session.title = event.SessionActivated.title;
    }
    state.historyLoaded.delete(agent);
    state.transcripts.set(agent, []);
    state.streaming.delete(agent);
    state.plans.delete(agent);
    state.usage.delete(agent);
    void loadHistory(agent, true);
  } else if (event.SessionClosed !== undefined) {
    const closedId = event.SessionClosed.id;
    state.sessions[agent] = (state.sessions[agent] ?? []).filter(
      (session) => session.id !== closedId,
    );
    if (state.activeSessions.get(agent) === closedId) {
      const next = latestSessionId(agent);
      if (next) state.activeSessions.set(agent, next);
      else state.activeSessions.delete(agent);
      state.historyLoaded.delete(agent);
      state.transcripts.set(agent, []);
      state.plans.delete(agent);
      state.usage.delete(agent);
      void loadHistory(agent, true);
    }
  } else if (event.SessionList !== undefined) {
    state.sessions[agent] = event.SessionList.sessions;
    const active = state.activeSessions.get(agent);
    if (!active || !event.SessionList.sessions.some((session: SessionInfo) => session.id === active)) {
      const next = latestSessionId(agent);
      if (next) state.activeSessions.set(agent, next);
      else state.activeSessions.delete(agent);
    }
    void loadHistory(agent);
  } else if (event.UsageUpdate !== undefined) {
    if (event.UsageUpdate.input_tokens !== null) {
      usage.input = event.UsageUpdate.input_tokens;
      usage.output = event.UsageUpdate.output_tokens;
      usage.cache = event.UsageUpdate.cache_read_input_tokens ?? 0;
      state.usage.set(agent, usage);
    }
    const current = state.streaming.get(agent);
    if (current) {
      current.usage = `输入 ${event.UsageUpdate.input_tokens ?? 0} · 输出 ${event.UsageUpdate.output_tokens} · 缓存 ${event.UsageUpdate.cache_read_input_tokens ?? 0}`;
    }
  } else if (event.LifecycleChanged !== undefined) {
    const lifecycle = event.LifecycleChanged;
    const statusName = (typeof lifecycle === "string"
      ? lifecycle
      : Object.keys(lifecycle)[0] ?? "").toLowerCase();
    state.statuses.set(agent, statusName);
    if (["idle", "cancelled", "error", "waiting", "aborted"].includes(statusName)) {
      state.streaming.delete(agent);
    }
  }
  if (import.meta.env.DEV && tag) {
    console.debug(tag, event);
  }
}

function handleFrame(frame: GatewayFrame) {
  if (frame.seq > state.lastSeq) {
    state.lastSeq = frame.seq;
  }
  if (frame.kind === "lag") {
    state.transcripts.clear();
    state.streaming.clear();
    void hydrate();
    return;
  }
  if (frame.kind === "agent") {
    const agent = frame.data.agent;
    handleAgentFrame(agent, frame.data.event);
  } else if (frame.kind === "host") {
    const view = frame.data;
    if (view.kind === "agent_registered") {
      if (!state.agents.some((agent) => agent.path === view.path)) {
        state.agents.push(view.info);
      }
    } else if (view.kind === "agent_unregistered") {
      state.agents = state.agents.filter((agent) => agent.path !== view.path);
    } else if (view.kind === "agent_status_changed") {
      const agent = state.agents.find((item) => item.path === view.path);
      if (agent) agent.status = view.status;
      state.statuses.set(view.path, normalizeStatus(view.status));
    }
  } else if (frame.kind === "notice") {
    if (frame.data.kind === "model_changed") {
      state.activeModel = frame.data.spec;
    } else if (frame.data.kind === "catalog_fetched") {
      const result = frame.data.result;
      const value = typeof result === "object" && result !== null ? result.Ok ?? result.err : result;
      const successful = typeof result === "object" && result !== null && result.Ok !== undefined;
      state.catalogBusy = null;
      state.configStatus = successful
        ? { kind: "ok", text: `Imported ${value} ${frame.data.provider} models` }
        : { kind: "error", text: `${frame.data.provider} catalogue failed: ${value}` };
      void refreshModelCatalog().finally(render);
    } else if (frame.data.kind === "chatgpt_login") {
      const result = frame.data.result;
      const info = typeof result === "object" && result !== null ? result.Ok : null;
      state.configStatus = info
        ? { kind: "ok", text: `ChatGPT login complete${info.email ? `: ${info.email}` : ""}` }
        : { kind: "error", text: `ChatGPT login failed: ${typeof result === "object" && result !== null ? result.Err : result}` };
      void refreshModelCatalog().finally(render);
    } else if (frame.data.kind === "kms_worker_changed") {
      const status = frame.data.status;
      const index = state.kmsWorkers.findIndex((worker) => worker.worker === status.worker);
      if (index >= 0) state.kmsWorkers[index] = status;
      else state.kmsWorkers.push(status);
    }
  }
  scheduleRender();
}

function sendDraft() {
  const draft = document.querySelector<HTMLTextAreaElement>("#composer")!;
  const text = draft.value.trim();
  if (!text || !state.activeAgent) return;
  pushPromptHistory(state.activeAgent, text);
  state.composerDrafts.delete(state.activeAgent);
  state.promptDrafts.delete(state.activeAgent);
  state.promptRecall.set(state.activeAgent, null);
  const search = state.historySearches.get(state.activeAgent);
  if (search?.active) closePromptSearch(state.activeAgent, draft, false);
  draft.value = "";
  render();
  request("POST", `/agents/${state.activeAgent}/messages`, { text })
    .catch((error) => appendItem(state.activeAgent, "error", String(error)))
    .finally(render);
}

function cancelAgent() {
  if (!state.activeAgent) return;
  request("POST", `/agents/${state.activeAgent}/cancel`)
    .catch((error) => appendItem(state.activeAgent, "error", String(error)))
    .finally(render);
}

function compactAgent() {
  if (!state.activeAgent) return;
  request("POST", `/agents/${state.activeAgent}/compact`)
    .catch((error) => appendItem(state.activeAgent, "error", String(error)))
    .finally(render);
}

function promptHistory(agent: string): string[] {
  let history = state.promptHistories.get(agent);
  if (!history) {
    history = [];
    state.promptHistories.set(agent, history);
  }
  return history;
}

function storeComposerDraft(agent: string, value?: string) {
  if (!agent) return;
  if (value !== undefined) {
    state.composerDrafts.set(agent, value);
    return;
  }
  const composer = document.querySelector<HTMLTextAreaElement>("#composer");
  if (composer && state.activeAgent === agent) state.composerDrafts.set(agent, composer.value);
}

function pushPromptHistory(agent: string, text: string) {
  if (!text.trim()) return;
  const history = promptHistory(agent);
  if (history.length >= 200) history.shift();
  history.push(text);
}

function historySearch(agent: string) {
  let search = state.historySearches.get(agent);
  if (!search) {
    search = { active: false, query: "", matches: [], selected: 0, draft: "" };
    state.historySearches.set(agent, search);
  }
  return search;
}

function computePromptMatches(history: string[], query: string): number[] {
  const needle = query.toLowerCase();
  return history
    .map((_, index) => index)
    .reverse()
    .filter((index) => !needle || history[index].toLowerCase().includes(needle));
}

function applyPromptMatch(agent: string, composer: HTMLTextAreaElement, search: ReturnType<typeof historySearch>) {
  const history = promptHistory(agent);
  const index = search.matches[search.selected];
  composer.value = index === undefined ? "" : history[index];
  composer.setSelectionRange(composer.value.length, composer.value.length);
}

function updatePromptSearch(agent: string, composer: HTMLTextAreaElement) {
  const search = historySearch(agent);
  search.matches = computePromptMatches(promptHistory(agent), search.query);
  search.selected = 0;
  applyPromptMatch(agent, composer, search);
  syncHistorySearchUi(agent, composer);
}

function startPromptSearch(agent: string, composer: HTMLTextAreaElement) {
  const history = promptHistory(agent);
  if (!history.length) return;
  const search = historySearch(agent);
  search.active = true;
  search.query = "";
  search.matches = [];
  search.selected = 0;
  search.draft = composer.value;
  state.promptRecall.set(agent, null);
  syncHistorySearchUi(agent, composer);
}

function closePromptSearch(agent: string, composer: HTMLTextAreaElement, restoreDraft: boolean) {
  const search = historySearch(agent);
  search.active = false;
  search.query = "";
  search.matches = [];
  search.selected = 0;
  if (restoreDraft) {
    composer.value = search.draft;
    composer.setSelectionRange(composer.value.length, composer.value.length);
  }
  search.draft = "";
  state.promptRecall.set(agent, null);
  syncHistorySearchUi(agent, composer);
}

function syncHistorySearchUi(agent: string, composer: HTMLTextAreaElement) {
  const search = state.historySearches.get(agent);
  const panel = document.querySelector<HTMLDivElement>("#history-search");
  if (!panel || !composer) return;
  if (!search?.active || state.activeAgent !== agent) {
    panel.hidden = true;
    panel.replaceChildren();
    return;
  }
  panel.hidden = false;
  panel.replaceChildren();
  const query = document.createElement("div");
  query.className = "search-query";
  query.textContent = `Ctrl+R ${search.query || "(type to filter)"}`;
  panel.append(query);
  const history = promptHistory(agent);
  search.matches.slice(0, 8).forEach((index, position) => {
    const row = document.createElement("button");
    row.type = "button";
    row.className = `search-match ${position === search.selected ? "active" : ""}`;
    row.textContent = history[index].replaceAll(/\s+/g, " ").slice(0, 220);
    row.onclick = () => {
      search.selected = position;
      applyPromptMatch(agent, composer, search);
      syncHistorySearchUi(agent, composer);
    };
    panel.append(row);
  });
  if (!search.matches.length) {
    const empty = document.createElement("div");
    empty.className = "muted";
    empty.textContent = "No matching prompts";
    panel.append(empty);
  }
  const help = document.createElement("footer");
  help.textContent = "type filter · Up older · Down newer · Enter accept · Esc cancel";
  panel.append(help);
}

function saveProviderKey(provider: string, key: string, baseUrl: string) {
  request("PUT", "/model-config/provider", {
    name: provider,
    provider_type: state.catalog.providers.find((item) => item.name === provider)?.provider_type ?? provider,
    base_url: baseUrl,
    api_key: key,
  })
    .then(async () => {
      await refreshModelCatalog();
      state.selectedProvider = "";
      state.apiKey = "";
      state.providerBaseUrl = "";
      state.configStatus = { kind: "ok", text: `Saved credentials for ${provider}` };
    })
    .catch((error) => {
      state.configStatus = { kind: "error", text: String(error) };
    })
    .finally(render);
}

async function refreshModelCatalog() {
  const snapshot = await request("GET", "/model-config");
  state.catalog = snapshot;
  state.activeModel = snapshot.active_model;
}

function newModelDraft(provider: string): CustomModelDraft {
  return {
    provider_name: provider,
    original_model_name: null,
    model_name: "",
    context_length: "128000",
    max_output_tokens: "8192",
    vision_ability: false,
    supports_function_calling: true,
    supports_streaming: true,
    supports_thinking: false,
    thinking_enabled: false,
    input_token_price: "0",
    output_token_price: "0",
  };
}

function openAddModel(provider: string) {
  state.modelForm = newModelDraft(provider);
  state.confirmDelete = null;
  state.configStatus = null;
  render();
}

function openEditModel(provider: string, modelName: string) {
  const model = providerModels(provider).find(
    (item) => item.model_name === modelName && item.custom,
  );
  if (!model) return;
  state.modelForm = {
    provider_name: provider,
    original_model_name: model.model_name,
    model_name: model.model_name,
    context_length: String(model.context_length),
    max_output_tokens: String(model.max_output_tokens),
    vision_ability: model.vision_ability,
    supports_function_calling: model.supports_function_calling,
    supports_streaming: model.supports_streaming,
    supports_thinking: model.supports_thinking,
    thinking_enabled: model.thinking_enabled,
    input_token_price: String(model.input_token_price),
    output_token_price: String(model.output_token_price),
  };
  state.confirmDelete = null;
  state.configStatus = null;
  render();
}

async function saveCustomModel() {
  const draft = state.modelForm;
  if (!draft) return;
  const contextLength = Number(draft.context_length);
  const maxOutput = Number(draft.max_output_tokens);
  const inputPrice = Number(draft.input_token_price);
  const outputPrice = Number(draft.output_token_price);
  const modelName = draft.model_name.trim();
  if (!modelName) {
    state.configStatus = { kind: "error", text: "Model name is required" };
    render();
    return;
  }
  if (!Number.isSafeInteger(contextLength) || contextLength <= 0) {
    state.configStatus = { kind: "error", text: "Context length must be a positive integer" };
    render();
    return;
  }
  if (!Number.isSafeInteger(maxOutput) || maxOutput <= 0) {
    state.configStatus = { kind: "error", text: "Max output tokens must be a positive integer" };
    render();
    return;
  }
  if (![inputPrice, outputPrice].every((price) => Number.isFinite(price) && price >= 0)) {
    state.configStatus = { kind: "error", text: "Prices must be numbers greater than or equal to zero" };
    render();
    return;
  }
  state.configStatus = { kind: "ok", text: "Saving model…" };
  render();
  try {
    await request("PUT", "/model-config/model", {
      provider_name: draft.provider_name,
      original_model_name: draft.original_model_name,
      model_name: modelName,
      context_length: contextLength,
      max_output_tokens: maxOutput,
      vision_ability: draft.vision_ability,
      supports_function_calling: draft.supports_function_calling,
      supports_streaming: draft.supports_streaming,
      supports_thinking: draft.supports_thinking,
      thinking_enabled: draft.thinking_enabled,
      input_token_price: inputPrice,
      output_token_price: outputPrice,
    });
    await refreshModelCatalog();
    state.modelForm = null;
    state.configStatus = {
      kind: "ok",
      text: `${draft.original_model_name ? "Updated" : "Added"} ${draft.provider_name}:${modelName}`,
    };
  } catch (error) {
    state.configStatus = { kind: "error", text: String(error) };
  } finally {
    render();
  }
}

async function deleteCustomModel(provider: string, modelName: string) {
  const spec = `${provider}:${modelName}`;
  if (state.confirmDelete !== spec) {
    state.confirmDelete = spec;
    render();
    return;
  }
  state.configStatus = { kind: "ok", text: "Deleting model…" };
  render();
  try {
    await request(
      "DELETE",
      `/model-config/model?provider_name=${encodeURIComponent(provider)}&model_name=${encodeURIComponent(modelName)}`,
    );
    await refreshModelCatalog();
    state.confirmDelete = null;
    state.configStatus = { kind: "ok", text: `Deleted ${spec}` };
  } catch (error) {
    state.configStatus = { kind: "error", text: String(error) };
  } finally {
    render();
  }
}

function supportsRemoteCatalog(providerType: string): boolean {
  return ["openai", "openrouter", "ollama"].includes(providerType);
}

async function fetchRemoteCatalog(provider: string, providerType: string, baseUrl: string) {
  if (!supportsRemoteCatalog(providerType)) {
    state.configStatus = { kind: "error", text: `${providerType} has no remote catalogue support` };
    render();
    return;
  }
  if (providerType === "openai" && !state.catalog.providers.find((item) => item.name === "openai")?.api_key.includes("refresh_token")) {
    state.configStatus = { kind: "error", text: "OpenAI catalogue requires ChatGPT login" };
    render();
    return;
  }
  state.catalogBusy = provider;
  state.configStatus = { kind: "ok", text: `Fetching ${provider} catalogue…` };
  render();
  try {
    await request(
      "POST",
      `/model-config/providers/${encodeURIComponent(provider)}/catalog`,
      { base_url: baseUrl },
    );
    state.configStatus = { kind: "ok", text: `Fetching ${provider} catalogue in daemon…` };
  } catch (error) {
    state.catalogBusy = null;
    state.configStatus = { kind: "error", text: String(error) };
    render();
  }
}

async function startChatgptLogin() {
  state.configStatus = { kind: "ok", text: "Starting ChatGPT login…" };
  render();
  try {
    const response = await request("POST", "/model-config/chatgpt/login");
    state.configStatus = {
      kind: "ok",
      text: `Authorize in the browser: ${response.url}`,
    };
    window.open(response.url, "_blank", "noopener");
  } catch (error) {
    state.configStatus = { kind: "error", text: String(error) };
  } finally {
    render();
  }
}

async function refreshChatgpt() {
  state.configStatus = { kind: "ok", text: "Refreshing ChatGPT token…" };
  render();
  try {
    const response = await request("POST", "/model-config/chatgpt/refresh");
    await refreshModelCatalog();
    state.configStatus = {
      kind: response.refreshed ? "ok" : "error",
      text: response.refreshed ? "ChatGPT token refreshed" : "ChatGPT token refresh was not needed or failed",
    };
  } catch (error) {
    state.configStatus = { kind: "error", text: String(error) };
  } finally {
    render();
  }
}

function spawnAgent() {
  const name = state.spawnName.trim();
  const parent = state.spawnParent.trim() || "/root";
  const profile = state.spawnProfile.trim();
  if (!name || !parent || !profile) return;
  request("POST", "/agents", { name, parent_path: parent, profile, model_spec: null })
    .then((result) => {
      state.spawnOpen = false;
      state.spawnName = "";
      if (result?.path && !state.agents.some((agent) => agent.path === result.path)) {
        state.agents.push({ path: result.path, agent_id: null, status: "idle" });
        storeComposerDraft(state.activeAgent);
        state.activeAgent = result.path;
      }
    })
    .catch((error) => window.alert(String(error)))
    .finally(render);
}

async function createSession(agent: string, title: string) {
  const cleanTitle = title.trim() || "New session";
  await request("POST", `/agents/${agent}/sessions`, {
    title: cleanTitle,
    fork_from: null,
    fork_message_id: null,
  });
  state.sessionCreating = false;
  state.sessionCreateTitle = "";
}

async function renameSession(agent: string, id: string, title: string) {
  const cleanTitle = title.trim() || "Untitled";
  await request("PATCH", `/agents/${agent}/sessions/${encodeURIComponent(id)}/title`, {
    title: cleanTitle,
  });
  state.sessionEditor = null;
}

async function closeSession(agent: string, id: string) {
  await request("POST", `/agents/${agent}/sessions/${encodeURIComponent(id)}/close`);
}

async function shutdownAgent(agent: string) {
  try {
    await request("POST", `/agents/${agent}/shutdown`);
    state.agents = state.agents.filter((item) => item.path !== agent);
    if (state.activeAgent === agent) state.activeAgent = state.agents[0]?.path ?? "";
  } catch (error) {
    window.alert(`Shutdown failed: ${error}`);
  } finally {
    render();
  }
}

async function deleteActiveAgent() {
  const agent = state.activeAgent;
  const info = state.agents.find((item) => item.path === agent);
  if (!agent || !info?.agent_id) return;
  if (!window.confirm(`Permanently delete ${agent}? Stored sessions and snapshots cannot be recovered.`)) return;
  try {
    await request("DELETE", `/storage/agents/${encodeURIComponent(info.agent_id)}`);
    await shutdownAgent(agent);
  } catch (error) {
    window.alert(`Delete failed: ${error}`);
  }
}

async function shutdownRunningAgent(agent: string) {
  if (state.agentBusy) return;
  if (!window.confirm(`Shutdown ${agent}? Stored history remains resumable.`)) return;
  state.agentBusy = agent;
  render();
  await shutdownAgent(agent);
  state.agentBusy = null;
  render();
}

async function deleteRunningAgent(agent: string, agentId: string | null) {
  if (state.agentBusy || !agentId) return;
  if (!window.confirm(`Permanently delete ${agent}? Stored sessions and snapshots cannot be recovered.`)) return;
  state.agentBusy = agent;
  render();
  try {
    await request("DELETE", `/storage/agents/${encodeURIComponent(agentId)}`);
    await request("POST", `/agents/${agent}/shutdown`);
    state.agents = state.agents.filter((item) => item.path !== agent);
    if (state.activeAgent === agent) state.activeAgent = state.agents[0]?.path ?? "";
  } catch (error) {
    window.alert(`Delete failed: ${error}`);
  } finally {
    state.agentBusy = null;
    render();
  }
}

async function loadStoredAgents(force = false) {
  if (state.resumePicker.loading && !force) return;
  state.resumePicker.loading = true;
  render();
  try {
    state.storedAgents = await request("GET", "/storage/agents");
  } catch (error) {
    window.alert(`Failed to load persisted agents: ${error}`);
  } finally {
    state.resumePicker.loading = false;
    render();
  }
}

function openResumePicker() {
  state.resumePicker.open = true;
  state.resumePicker.query = "";
  state.resumePicker.selected = 0;
  state.renameAgent = null;
  state.deleteAgent = null;
  void loadStoredAgents(true);
  render();
}

function closeResumePicker() {
  state.resumePicker.open = false;
  state.renameAgent = null;
  state.deleteAgent = null;
  render();
}

function filteredStoredAgents(): StoredAgent[] {
  const needle = state.resumePicker.query.trim().toLowerCase();
  return state.storedAgents
    .filter((agent) => !needle || `${agent.name} ${agent.config_json.kind ?? ""}`.toLowerCase().includes(needle))
    .sort((left, right) => right.last_active - left.last_active);
}

function leafName(path: string): string {
  return path.split("/").filter(Boolean).pop() ?? "";
}

function parentPath(path: string): string {
  const segments = path.split("/").filter(Boolean);
  return segments.length <= 1 ? "/root" : `/${segments.slice(0, -1).join("/")}`;
}

async function resumeStoredAgent(agent: StoredAgent) {
  if (state.resumeBusy) return;
  const running = state.agents.find((item) => item.agent_id === agent.id);
  if (running) {
    storeComposerDraft(state.activeAgent);
    state.activeAgent = running.path;
    state.resumePicker.open = false;
    void loadHistory(running.path, true);
    render();
    return;
  }
  const name = leafName(agent.name);
  if (!name) {
    window.alert("Cannot resume the root placeholder.");
    return;
  }
  state.resumeBusy = agent.id;
  render();
  try {
    const path = await request("POST", "/agents", {
      name,
      parent_path: parentPath(agent.name),
      profile: agent.config_json.kind ?? "researcher",
      runtime: agent.config_json.runtime ?? {},
      model_spec: agent.config_json.preferred_model ?? null,
    });
    state.activeAgent = path.path;
    state.resumePicker.open = false;
    state.transcripts.delete(path.path);
    state.historyLoaded.delete(path.path);
    void loadHistory(path.path, true);
  } catch (error) {
    window.alert(`Resume failed: ${error}`);
  } finally {
    state.resumeBusy = null;
    render();
  }
}

function validateAgentSegment(name: string): string | null {
  if (!name) return "Name is required";
  if (name === "root" || name === "." || name === "..") return `"${name}" is reserved`;
  if (name.length > 32) return "Name must be at most 32 characters";
  if (!/^[a-z0-9_]+$/.test(name)) return "Use lowercase letters, digits, and underscore only";
  return null;
}

async function renameStoredAgent() {
  const editing = state.renameAgent;
  const agent = state.storedAgents.find((item) => item.id === editing?.id);
  if (!editing || !agent) return;
  const error = validateAgentSegment(editing.name);
  if (error) {
    editing.error = error;
    render();
    return;
  }
  const segments = agent.name.split("/").filter(Boolean);
  const newPath = segments.length <= 1 ? `/root/${editing.name}` : `/${[...segments.slice(0, -1), editing.name].join("/")}`;
  state.resumeBusy = agent.id;
  render();
  try {
    await request("PATCH", `/storage/agents/${encodeURIComponent(agent.id)}`, { path: newPath });
    agent.name = newPath;
    state.renameAgent = null;
  } catch (error) {
    window.alert(`Rename failed: ${error}`);
  } finally {
    state.resumeBusy = null;
    render();
  }
}

async function deleteStoredAgent() {
  const confirmation = state.deleteAgent;
  const agent = state.storedAgents.find((item) => item.id === confirmation?.id);
  if (!confirmation || !agent) return;
  if (confirmation.confirm.trim().toLowerCase() !== "yes") return;
  state.resumeBusy = agent.id;
  render();
  try {
    await request("DELETE", `/storage/agents/${encodeURIComponent(agent.id)}`);
    state.storedAgents = state.storedAgents.filter((item) => item.id !== agent.id);
    const running = state.agents.find((item) => item.agent_id === agent.id);
    if (running) {
      await request("POST", `/agents/${running.path}/shutdown`);
      state.agents = state.agents.filter((item) => item.path !== running.path);
      if (state.activeAgent === running.path) {
        state.activeAgent = state.agents[0]?.path ?? "";
      }
    }
    state.deleteAgent = null;
  } catch (error) {
    window.alert(`Delete failed: ${error}`);
  } finally {
    state.resumeBusy = null;
    render();
  }
}

function setDefaultModel(spec: string) {
  request("PUT", "/model-config/active-model", { spec })
    .then(() => {
      state.activeModel = spec;
    })
    .catch((error) => window.alert(String(error)))
    .finally(render);
}

async function loadAgentConfig(agent: string, force = false) {
  if (!agent) return;
  if (state.agentConfigLoadedFor === agent && !force) return;
  if (state.agentConfigLoadedFor !== agent) {
    state.agentRuntime = {};
    state.agentEffective = null;
    state.agentConfigError = null;
  }
  state.agentConfigLoading = true;
  state.agentConfigLoadedFor = agent;
  render();
  try {
    const view: AgentRuntimeConfigView = await request("GET", `/agents/${agent}/config`);
    if (state.agentConfigLoadedFor === agent) {
      state.agentRuntime = view.runtime;
      state.agentEffective = view.effective;
      state.agentConfigError = null;
    }
  } catch (error) {
    if (state.agentConfigLoadedFor === agent) state.agentConfigError = String(error);
  } finally {
    state.agentConfigLoading = false;
    render();
  }
}

async function saveAgentConfig() {
  if (!state.activeAgent || state.agentConfigSaving) return;
  state.agentConfigSaving = true;
  render();
  try {
    const view: AgentRuntimeConfigView = await request("PUT", `/agents/${state.activeAgent}/config`, {
      runtime: state.agentRuntime,
    });
    state.agentRuntime = view.runtime;
    state.agentEffective = view.effective;
    state.agentConfigError = null;
  } catch (error) {
    state.agentConfigError = String(error);
  } finally {
    state.agentConfigSaving = false;
    render();
  }
}

async function saveDisplaySetting(key: keyof typeof state.display, value: boolean) {
  const previous = state.display[key];
  state.display[key] = value;
  render();
  try {
    await request("PUT", "/settings", { key, value: value ? "1" : "0" });
  } catch (error) {
    state.display[key] = previous;
    window.alert(String(error));
    render();
  }
}

async function loadEmbeddingConfig(force = false) {
  if ((state.embeddingLoading || state.embeddingSaving) && !force) return;
  state.embeddingLoading = true;
  render();
  try {
    state.embedding = await request("GET", "/embedding-config");
    state.embeddingError = null;
  } catch (error) {
    state.embeddingError = String(error);
  } finally {
    state.embeddingLoading = false;
    render();
  }
}

async function saveEmbeddingConfig() {
  if (state.embeddingSaving) return;
  state.embeddingSaving = true;
  render();
  try {
    const response = await request("PUT", "/embedding-config", { config: state.embedding });
    state.embedding = response.config ?? state.embedding;
    state.embeddingError = null;
  } catch (error) {
    state.embeddingError = String(error);
  } finally {
    state.embeddingSaving = false;
    render();
  }
}

async function loadDag(agent: string, force = false) {
  if (!agent) {
    state.dagSnapshot = null;
    state.dagLoadedFor = null;
    state.dagError = "DAG view is scoped to an agent.";
    return;
  }
  if (state.dagLoading || (state.dagLoadedFor === agent && !force)) return;
  state.dagLoading = true;
  state.dagLoadedFor = agent;
  state.dagError = null;
  render();
  try {
    const snapshot: DagSnapshot = await request("GET", `/agents/${agent}/dag`);
    state.dagSnapshot = snapshot;
    if (!snapshot.nodes.some((node) => node.id === state.dagSelected)) {
      state.dagSelected = snapshot.nodes[0]?.id ?? null;
    }
  } catch (error) {
    state.dagSnapshot = null;
    state.dagError = String(error);
  } finally {
    state.dagLoading = false;
    render();
  }
}

function dagStatus(node: DagSnapshot["nodes"][number]): string {
  if (typeof node.status === "string") return node.status.toLowerCase();
  if (node.status && typeof node.status === "object") {
    return Object.keys(node.status)[0]?.toLowerCase() ?? "pending";
  }
  return "pending";
}

function dagLevels(snapshot: DagSnapshot): Map<string, number> {
  const incoming = new Map(snapshot.nodes.map((node) => [node.id, 0]));
  const outgoing = new Map(snapshot.nodes.map((node) => [node.id, [] as string[]]));
  for (const edge of snapshot.edges) {
    incoming.set(edge.to, (incoming.get(edge.to) ?? 0) + 1);
    outgoing.get(edge.from)?.push(edge.to);
  }
  const levels = new Map(snapshot.nodes.map((node) => [node.id, 0]));
  const queue = snapshot.nodes.filter((node) => (incoming.get(node.id) ?? 0) === 0).map((node) => node.id);
  const remaining = new Map(incoming);
  while (queue.length) {
    const id = queue.shift()!;
    for (const next of outgoing.get(id) ?? []) {
      levels.set(next, Math.max(levels.get(next) ?? 0, (levels.get(id) ?? 0) + 1));
      const count = (remaining.get(next) ?? 1) - 1;
      remaining.set(next, count);
      if (count === 0) queue.push(next);
    }
  }
  return levels;
}

function renderDag(container: HTMLDivElement) {
  container.className = "transcript dag";
  container.replaceChildren();
  const snapshot = state.dagSnapshot;
  const toolbar = document.createElement("div");
  toolbar.className = "dag-toolbar";
  const counts = snapshot
    ? [
        `${snapshot.nodes.length} nodes`,
        `${snapshot.edges.length} edges`,
        `${snapshot.nodes.filter((node) => dagStatus(node) === "running").length} running`,
        `${snapshot.nodes.filter((node) => dagStatus(node) === "failed").length} failed`,
      ].join("    ")
    : `agent ${state.activeAgent || "—"}`;
  toolbar.innerHTML = `<span>${escapeHtml(counts)}</span>`;
  const refresh = document.createElement("button");
  refresh.textContent = "Refresh";
  refresh.onclick = () => void loadDag(state.activeAgent, true);
  toolbar.append(refresh);
  container.append(toolbar);

  if (state.dagLoading) {
    const loading = document.createElement("div");
    loading.className = "bib-status";
    loading.textContent = "Loading DAG snapshot…";
    container.append(loading);
    return;
  }
  if (state.dagError) {
    const error = document.createElement("div");
    error.className = "bib-status error";
    error.textContent = state.dagError;
    container.append(error);
    return;
  }
  if (!snapshot) {
    container.insertAdjacentHTML("beforeend", `<div class="empty">No DAG snapshot loaded</div>`);
    return;
  }
  if (!snapshot.nodes.length) {
    container.insertAdjacentHTML(
      "beforeend",
      `<div class="empty">This agent has no live dataflow DAG.</div>`,
    );
    return;
  }

  const levels = dagLevels(snapshot);
  const maxLevel = Math.max(0, ...levels.values());
  const columns = new Map<number, string[]>();
  for (const node of snapshot.nodes) {
    const level = levels.get(node.id) ?? 0;
    const column = columns.get(level) ?? [];
    column.push(node.id);
    columns.set(level, column);
  }
  const width = Math.max(container.clientWidth - 36, (maxLevel + 1) * 250 + 48);
  const height = Math.max(280, Math.max(...[...columns.values()].map((rows) => rows.length)) * 100 + 56);
  const canvas = document.createElement("div");
  canvas.className = "dag-canvas";
  canvas.style.width = `${width}px`;
  canvas.style.height = `${height}px`;
  const svg = document.createElementNS("http://www.w3.org/2000/svg", "svg");
  svg.setAttribute("width", String(width));
  svg.setAttribute("height", String(height));
  const selectedEdges = new Set(
    snapshot.edges
      .filter((edge) => edge.from === state.dagSelected || edge.to === state.dagSelected)
      .map((edge) => `${edge.from}->${edge.to}`),
  );
  const positions = new Map<string, { x: number; y: number }>();
  for (const [level, ids] of columns) {
    ids.forEach((id, row) => {
      positions.set(id, { x: 28 + level * 250, y: 28 + row * 100 });
    });
  }
  for (const edge of snapshot.edges) {
    const from = positions.get(edge.from);
    const to = positions.get(edge.to);
    if (!from || !to) continue;
    const path = document.createElementNS("http://www.w3.org/2000/svg", "path");
    const start = from.x + 210;
    const end = to.x;
    const bend = (end - start) / 2;
    path.setAttribute(
      "d",
      `M ${start} ${from.y + 36} C ${start + bend} ${from.y + 36}, ${end - bend} ${to.y + 36}, ${end} ${to.y + 36}`,
    );
    path.setAttribute("class", `dag-edge ${selectedEdges.has(`${edge.from}->${edge.to}`) ? "active" : ""}`);
    svg.append(path);
  }
  canvas.append(svg);
  for (const node of snapshot.nodes) {
    const position = positions.get(node.id);
    if (!position) continue;
    const button = document.createElement("button");
    button.className = `dag-node status-${dagStatus(node)} ${state.dagSelected === node.id ? "selected" : ""}`;
    button.style.left = `${position.x}px`;
    button.style.top = `${position.y}px`;
    button.innerHTML = `
      <strong>${escapeHtml(node.id)}</strong>
      <span>${escapeHtml(node.kind)}</span>
      <small>${escapeHtml(dagStatus(node))}${node.dirty ? " · dirty" : ""}</small>`;
    button.onclick = () => {
      state.dagSelected = node.id;
      render();
    };
    canvas.append(button);
  }
  const scroll = document.createElement("div");
  scroll.className = "dag-scroll";
  scroll.append(canvas);
  container.append(scroll);

  const selected = snapshot.nodes.find((node) => node.id === state.dagSelected);
  const detail = document.createElement("article");
  detail.className = "browser-detail";
  if (selected) {
    const heading = document.createElement("h3");
    heading.textContent = selected.id;
    const meta = document.createElement("pre");
    meta.textContent = [
      `Kind: ${selected.kind}`,
      `Status: ${dagStatus(selected)}`,
      `Cache: ${selected.dirty ? "dirty" : "cached"}`,
      `Inputs: ${selected.inputs.map((port) => `[${port.index}] ${port.label || port.data_type}`).join(", ") || "—"}`,
      `Outputs: ${selected.outputs.map((port) => `[${port.index}] ${port.label || port.data_type}`).join(", ") || "—"}`,
    ].join("\n");
    detail.append(heading, meta);
  } else {
    detail.innerHTML = `<div class="empty">No node selected</div>`;
  }
  container.append(detail);
}

function providerModels(provider: string) {
  return state.catalog.models.filter((model) => model.provider_name === provider);
}

function bibArticles(): BibArticle[] {
  const collection = state.bibCollections.find((item) => item.id === state.bibSelectedCollection);
  if (!collection) return state.bibArticles;
  const memberIds = new Set(collection.article_ids);
  return state.bibArticles.filter((article) => memberIds.has(article.id));
}

function bibIdentifier(article: BibArticle, kind: string): string {
  return article.identifiers?.find((item) => item.kind.toLowerCase() === kind)?.value ?? "";
}

function bibAuthors(article: BibArticle): string {
  return (article.authors ?? [])
    .map((author) => author.name || [author.fore_name, author.last_name].filter(Boolean).join(" "))
    .filter(Boolean)
    .join(", ");
}

function bibAnnotationLabel(note: BibAnnotation): string {
  return typeof note.kind === "string" ? note.kind : "note";
}

async function loadMoreBibFullText(articleId: string) {
  const pagination = state.bibDetail?.fulltext_pagination;
  if (!pagination?.next_offset || state.bibDetailLoading) return;
  state.bibDetailLoading = true;
  render();
  try {
    const next = await request(
      "GET",
      `/bib/articles/${encodeURIComponent(articleId)}?offset=${pagination.next_offset}`,
    );
    if (state.bibDetail && state.bibSelectedArticle === articleId) {
      const previous = state.bibDetail.fulltext?.text_content ?? "";
      const appended = next.fulltext?.text_content ?? "";
      state.bibDetail.fulltext = {
        ...(state.bibDetail.fulltext ?? {}),
        ...(next.fulltext ?? {}),
        text_content: previous + appended,
      };
      state.bibDetail.fulltext_pagination = next.fulltext_pagination;
    }
  } catch (error) {
    state.bibError = String(error);
  } finally {
    state.bibDetailLoading = false;
    render();
  }
}

function mergeKmsRows(library: SkillLibraryRow[], proposals: KmsProposal[]): KmsRow[] {
  const proposalByName = new Map(proposals.map((proposal) => [proposal.skill_name, proposal]));
  const rows: KmsRow[] = library.map((skill) => {
    const proposal = proposalByName.get(skill.name) ?? null;
    return {
      key: skill.name,
      name: skill.name,
      tier: skill.tier,
      description: skill.description,
      tags: skill.tags,
      body: "",
      installed: skill.installed,
      proposal_status: proposal?.status ?? skill.proposal_status,
      proposal_update: proposal ? skill.installed : skill.proposal_update,
      proposal,
    };
  });
  const seen = new Set(rows.map((row) => row.name));
  for (const proposal of proposals) {
    if (seen.has(proposal.skill_name)) continue;
    rows.push({
      key: proposal.skill_name,
      name: proposal.skill_name,
      tier: "proposed",
      description: proposal.title,
      tags: [],
      body: proposal.body_md,
      installed: false,
      proposal_status: proposal.status,
      proposal_update: false,
      proposal,
    });
  }
  return rows.sort((left, right) => {
    const pending = (row: KmsRow) => (row.proposal?.status !== "pending" ? 1 : 0);
    return pending(left) - pending(right) || left.name.localeCompare(right.name);
  });
}

async function loadKms(force = false) {
  if ((state.kmsLoaded || state.kmsLoading) && !force) return;
  state.kmsLoading = true;
  render();
  const tasks = [
    request("GET", "/kms/workers"),
    request("GET", "/skills/library"),
    requestOptional("/kms/knowledge"),
    requestOptional("/kms/skill-proposals"),
    requestOptional("/kms/evidence"),
  ] as const;
  const [workers, library, knowledge, proposals, evidence] = await Promise.allSettled(tasks);
  const failures = [workers, library]
    .filter((item) => item.status === "rejected")
    .map((item) => String((item as PromiseRejectedResult).reason));
  if (workers.status === "fulfilled") state.kmsWorkers = workers.value;
  if (library.status === "fulfilled") state.kmsLibrary = library.value;
  if (knowledge.status === "fulfilled" && knowledge.value) {
    state.kmsKnowledge = [...knowledge.value].sort((left, right) =>
      left.path.localeCompare(right.path),
    );
  }
  if (proposals.status === "fulfilled" && proposals.value) state.kmsProposals = proposals.value;
  if (evidence.status === "fulfilled" && evidence.value) state.kmsEvidence = evidence.value;
  state.kmsRows = mergeKmsRows(state.kmsLibrary, state.kmsProposals);
  if (!state.kmsRows.some((row) => row.name === state.kmsSelectedSkill)) {
    state.kmsSelectedSkill =
      state.kmsRows.find((row) => row.proposal?.status === "pending")?.name ??
      state.kmsRows[0]?.name ??
      null;
  }
  if (!state.kmsKnowledge.some((node) => node.id === state.kmsSelectedKnowledge)) {
    state.kmsSelectedKnowledge = state.kmsKnowledge[0]?.id ?? null;
  }
  if (!state.kmsEvidence.some((row) => row.id === state.kmsSelectedEvidence)) {
    state.kmsSelectedEvidence = state.kmsEvidence[0]?.id ?? null;
  }
  state.kmsError = failures.length ? failures.join("; ") : null;
  state.kmsLoaded = !failures.length;
  state.kmsLoading = false;
  await loadKmsSkillDetail(state.kmsSelectedSkill);
  render();
}

async function loadKmsSkillDetail(name: string | null) {
  if (!name) {
    state.kmsSkillDetail = null;
    return;
  }
  state.kmsSkillDetailLoading = true;
  render();
  try {
    state.kmsSkillDetail = await requestOptional(`/skills/library/${encodeURIComponent(name)}`);
  } catch (error) {
    state.kmsSkillDetail = null;
    state.kmsError = String(error);
  } finally {
    state.kmsSkillDetailLoading = false;
    render();
  }
}

async function decideKmsProposal(approved: boolean) {
  const row = state.kmsRows.find((item) => item.name === state.kmsSelectedSkill);
  const proposal = row?.proposal;
  if (!proposal || proposal.status !== "pending" || state.kmsActionBusy) return;
  state.kmsActionBusy = true;
  render();
  try {
    const decision = await request(
      "POST",
      `/kms/skill-proposals/${proposal.id}/${approved ? "approve" : "reject"}`,
    );
    state.kmsLastReport = `${approved ? "approved" : "rejected"} ${decision.skill_name}${
      decision.skill_version_id ? ` (version ${decision.skill_version_id})` : ""
    }`;
    await loadKms(true);
  } catch (error) {
    state.kmsError = String(error);
  } finally {
    state.kmsActionBusy = false;
    render();
  }
}

function renderBibliography(container: HTMLDivElement) {
  container.className = "transcript bib";
  container.replaceChildren();

  const toolbar = document.createElement("div");
  toolbar.className = "bib-toolbar";
  const form = document.createElement("form");
  const input = document.createElement("input");
  input.value = state.bibQueryDraft;
  input.placeholder = `Search ${state.bibTotal.toLocaleString()} articles`;
  input.oninput = () => {
    state.bibQueryDraft = input.value;
  };
  const search = document.createElement("button");
  search.type = "submit";
  search.textContent = "Search";
  const reload = document.createElement("button");
  reload.type = "button";
  reload.textContent = "Reload";
  reload.onclick = () => void loadBibliography(true);
  form.append(input, search);
  const exportControls = document.createElement("div");
  exportControls.className = "bib-export";
  const format = document.createElement("select");
  for (const entry of [
    ["bibtex", "BibTeX"],
    ["ris", "RIS"],
    ["markdown", "Markdown"],
    ["csl_json", "CSL JSON"],
  ] as const) {
    const option = document.createElement("option");
    option.value = entry[0];
    option.textContent = entry[1];
    option.selected = state.bibExportFormat === entry[0];
    format.append(option);
  }
  format.onchange = () => {
    state.bibExportFormat = format.value as typeof state.bibExportFormat;
  };
  const exportButton = document.createElement("button");
  exportButton.type = "button";
  exportButton.textContent = state.bibExportBusy ? "Exporting…" : "Export";
  exportButton.disabled = state.bibExportBusy;
  exportButton.onclick = () => void exportBibliography();
  exportControls.append(format, exportButton);
  const workflow = document.createElement("div");
  workflow.className = "bib-export";
  const requests = document.createElement("button");
  requests.type = "button";
  requests.textContent = "Requests";
  requests.onclick = openBibRequests;
  const importButton = document.createElement("button");
  importButton.type = "button";
  importButton.textContent = "Import";
  importButton.onclick = openBibImport;
  workflow.append(requests, importButton);
  toolbar.append(form, exportControls, workflow, reload);
  form.onsubmit = (event) => {
    event.preventDefault();
    state.bibQuery = state.bibQueryDraft;
    state.bibLoaded = false;
    void loadBibliography(true);
  };

  const collections = document.createElement("div");
  collections.className = "bib-collections";
  const all = document.createElement("button");
  all.className = `bib-row ${state.bibSelectedCollection === null ? "active" : ""}`;
  all.innerHTML = `<strong>All articles</strong><small>${state.bibTotal.toLocaleString()}</small>`;
  all.onclick = () => {
    state.bibSelectedCollection = null;
    state.bibSelectedArticle = bibArticles()[0]?.id ?? null;
    void loadBibliographyDetail(state.bibSelectedArticle);
    render();
  };
  collections.append(all);
  for (const collection of state.bibCollections) {
    const row = document.createElement("button");
    row.className = `bib-row ${state.bibSelectedCollection === collection.id ? "active" : ""}`;
    row.title = collection.description || collection.name;
    row.innerHTML = `<strong>${escapeHtml(collection.name)}</strong><small>${collection.article_ids.length.toLocaleString()}</small>`;
    row.onclick = () => {
      state.bibSelectedCollection = collection.id;
      state.bibSelectedArticle = bibArticles()[0]?.id ?? null;
      void loadBibliographyDetail(state.bibSelectedArticle);
      render();
    };
    collections.append(row);
  }

  const articles = document.createElement("div");
  articles.className = "bib-articles";
  const visible = bibArticles();
  for (const article of visible) {
    const row = document.createElement("button");
    row.className = `bib-row article ${state.bibSelectedArticle === article.id ? "active" : ""}`;
    row.innerHTML = `
      <div class="bib-title">${escapeHtml(article.title)}</div>
      <small>${escapeHtml(article.year ?? "----")} · ${escapeHtml(bibAuthors(article) || article.source || "unknown")}</small>`;
    row.onclick = () => {
      state.bibSelectedArticle = article.id;
      state.bibDetailTab = "metadata";
      void loadBibliographyDetail(article.id);
      render();
    };
    articles.append(row);
  }
  if (!visible.length) {
    articles.innerHTML = `<div class="empty">No matching articles</div>`;
  }

  const details = document.createElement("article");
  details.className = "bib-details";
  const tabs = document.createElement("div");
  tabs.className = "detail-tabs";
  const selected = state.bibArticles.find((item) => item.id === state.bibSelectedArticle)
    ?? state.bibDetail?.article ?? null;
  for (const entry of [
    ["metadata", "Metadata"],
    ["fulltext", "Full text"],
    ["notes", "Notes"],
  ] as const) {
    const tab = document.createElement("button");
    tab.className = state.bibDetailTab === entry[0] ? "active" : "";
    tab.textContent = entry[1];
    tab.onclick = () => {
      state.bibDetailTab = entry[0];
      render();
    };
    tabs.append(tab);
  }
  if (selected && state.bibDetail?.fulltext?.file_format?.toLowerCase() === "pdf") {
    const read = document.createElement("div");
    read.className = "pdf-reader-entry";
    const button = document.createElement("button");
    button.className = "ghost full primary";
    button.textContent = "Read PDF";
    button.onclick = () => {
      void openPdfReader({
        articleId: selected.id,
        title: selected.title,
      }).catch((error) => {
        state.bibError = String(error);
        render();
      });
    };
    read.append(button);
    details.append(read);
  }
  const detailBody = document.createElement("pre");
  if (!selected) {
    detailBody.textContent = "No article selected";
  } else if (state.bibDetailLoading) {
    detailBody.textContent = "Loading…";
  } else if (state.bibDetailTab === "metadata") {
    detailBody.textContent = [
      `Title: ${selected.title}`,
      `Authors: ${bibAuthors(selected) || "—"}`,
      `Year: ${selected.year ?? "—"}`,
      `Journal: ${selected.journal ?? "—"}`,
      `DOI: ${bibIdentifier(selected, "doi") || "—"}`,
      `PMID: ${bibIdentifier(selected, "pmid") || "—"}`,
      `Source: ${selected.source ?? "—"}`,
      `Keywords: ${(selected.keywords ?? []).join(", ") || "—"}`,
      "",
      "Abstract",
      selected.abstract_text || "Not available",
    ].join("\n");
  } else if (state.bibDetailTab === "fulltext") {
    const fulltext = state.bibDetail?.fulltext;
    detailBody.textContent = fulltext?.text_content
      ? fulltext.text_content + (state.bibDetail?.fulltext_pagination?.truncated ? "\n\n[truncated]" : "")
      : [
          "No extracted text",
          fulltext?.file_path ? `File: ${fulltext.file_path}` : "Article ID: " + selected.id,
          `Extraction: ${fulltext?.extract_status ?? "not uploaded"}`,
        ].join("\n");
  } else {
    const notes = state.bibDetail?.annotations ?? [];
    detailBody.textContent = notes.length
      ? notes.map((note) => `[${bibAnnotationLabel(note)}] ${note.content}`).join("\n\n")
      : "No notes yet";
  }
  details.append(tabs, detailBody);
  if (
    state.bibDetailTab === "fulltext" &&
    state.bibDetail?.fulltext_pagination?.next_offset &&
    state.bibSelectedArticle
  ) {
    const more = document.createElement("button");
    more.className = "ghost full";
    more.textContent = state.bibDetailLoading ? "Loading…" : "Load more";
    more.disabled = state.bibDetailLoading;
    const articleId = state.bibSelectedArticle;
    more.onclick = () => void loadMoreBibFullText(articleId);
    details.append(more);
  }
  if (selected) {
    const upload = document.createElement("div");
    upload.className = "bib-upload";
    const hint = document.createElement("p");
    hint.textContent = state.bibUploadBusy
      ? "Uploading full text…"
      : "Upload PDF, HTML, or TXT full text; extraction runs in the daemon.";
    const button = document.createElement("button");
    button.className = "ghost full";
    button.textContent = state.bibUploadBusy ? "Uploading…" : "Upload full text";
    button.disabled = state.bibUploadBusy;
    const articleId = selected.id;
    button.onclick = () => void uploadBibliographyFulltext(articleId);
    upload.append(hint, button);
    details.append(upload);
  }

  if (state.bibLoading) {
    const loading = document.createElement("div");
    loading.className = "bib-status";
    loading.textContent = "Loading bibliography…";
    container.append(loading);
  }
  if (state.bibError) {
    const error = document.createElement("div");
    error.className = "bib-status error";
    error.textContent = state.bibError;
    container.append(error);
  }
  container.append(toolbar, collections, articles, details);
}

function statusLabel(agent: string): string {
  const live = state.statuses.get(agent);
  return normalizeStatus(live ?? state.agents.find((item) => item.path === agent)?.status);
}

function normalizeStatus(value: unknown): string {
  if (typeof value === "string") return value.toLowerCase();
  if (value && typeof value === "object" && "kind" in value) {
    return String((value as { kind: unknown }).kind).toLowerCase();
  }
  return "idle";
}

function renderTranscript(container: HTMLDivElement) {
  const transcript = transcriptFor(state.activeAgent);
  const previousScrollTop = container.scrollTop;
  const follow = state.autoScroll.get(state.activeAgent) ?? true;
  const savedScrollTop = state.scrollOffsets.get(state.activeAgent) ?? 0;
  container.replaceChildren();
  if (!state.activeAgent) {
    const empty = document.createElement("div");
    empty.className = "empty";
    empty.textContent = "没有活动的 agent。daemon 恢复后这里会自动同步。";
    container.append(empty);
    return;
  }
  let hiddenThinking = 0;
  let hiddenToolCalls = 0;
  let hiddenToolResults = 0;
  for (const item of transcript) {
    if (item.role === "thinking" && state.display.collapse_thinking) {
      hiddenThinking += 1;
      continue;
    }
    if (item.role === "tool" && item.kind === "tool_call" && state.display.collapse_tool_calls) {
      hiddenToolCalls += 1;
      continue;
    }
    if (item.role === "tool" && item.kind === "tool_result" && state.display.collapse_tool_results) {
      hiddenToolResults += 1;
      continue;
    }
    const article = document.createElement("article");
    article.className = `message ${item.role}`;
    const label = document.createElement("span");
    label.className = "message-label";
    label.textContent = { user: "You", assistant: "Agent", thinking: "Thinking", tool: "Tool", error: "Error", status: "Status" }[item.role];
    if (item.role === "user" || item.role === "assistant") {
      const copy = document.createElement("button");
      copy.className = "message-copy";
      copy.textContent = "Copy";
      copy.onclick = () => void copyText(item.text);
      label.append(copy);
    }
    const body = document.createElement(item.role === "assistant" ? "div" : "pre");
    if (item.role === "assistant") {
      body.className = "markdown";
      body.innerHTML = renderMarkdown(item.text);
    } else {
      body.textContent = item.text;
    }
    article.append(label, body);
    if (item.usage) {
      const usage = document.createElement("footer");
      usage.textContent = item.usage;
      article.append(usage);
    }
    container.append(article);
  }
  const summary = [
    hiddenThinking ? `${hiddenThinking} thinking collapsed` : "",
    hiddenToolCalls ? `${hiddenToolCalls} tool calls collapsed` : "",
    hiddenToolResults ? `${hiddenToolResults} results collapsed` : "",
  ].filter(Boolean).join(" · ");
  if (summary) {
    const footer = document.createElement("footer");
    footer.className = "collapse-summary";
    footer.textContent = summary;
    container.append(footer);
  }
  container.addEventListener("scroll", () => {
    state.scrollOffsets.set(state.activeAgent, container.scrollTop);
  });
  container.scrollTop = follow
    ? container.scrollHeight
    : previousScrollTop || savedScrollTop;
}

function selectableMessages(): ChatItem[] {
  return transcriptFor(state.activeAgent)
    .filter((item) => (item.role === "user" || item.role === "assistant") && item.text.trim())
    .reverse();
}

type PickerForkItem = {
  id: string;
  role: "user" | "assistant";
  text: string;
};

function rawKeyForActiveSession(): string | null {
  const sessionId = state.activeSessions.get(state.activeAgent);
  return sessionId ? `${state.activeAgent}:${sessionId}` : null;
}

function forkableMessages(): PickerForkItem[] {
  const key = rawKeyForActiveSession();
  if (!key) return [];
  return (state.rawMessages.get(key) ?? [])
    .map((message) => {
      const text = (message.content ?? [])
        .filter((block) => block.type === "text" && block.text)
        .map((block) => block.text ?? "")
        .join("\n");
      return {
        id: message.id,
        role: message.role,
        text: text.trim() || `${message.role} message`,
      };
    })
    .reverse();
}

function messagePreview(text: string): string {
  const first = text.split(/\r?\n/).find((line) => line.trim()) ?? "";
  const collapsed = first.split(/\s+/).filter(Boolean).join(" ");
  return collapsed.length <= 100 ? collapsed : `${collapsed.slice(0, 99)}…`;
}

async function openForkMessagePicker() {
  const agent = state.activeAgent;
  if (!agent || !state.activeSessions.get(agent)) {
    showCopyStatus("Fork requires an active session");
    return;
  }
  state.messagePicker = { open: true, mode: "fork", query: "", selected: 0 };
  render();
  await loadHistory(agent, true);
  if (!forkableMessages().length) {
    state.messagePicker.open = false;
    showCopyStatus("No forkable backend messages available");
    render();
    return;
  }
  render();
}

async function forkFromMessage(message: PickerForkItem) {
  const agent = state.activeAgent;
  const sessionId = state.activeSessions.get(agent);
  const info = state.agents.find((item) => item.path === agent);
  if (!agent || !sessionId || !info?.agent_id || state.forkBusy) return;
  state.forkBusy = true;
  render();
  try {
    await request("POST", `/agents/${agent}/sessions`, {
      title: `Fork at ${messagePreview(message.text)}`,
      fork_from: sessionId,
      fork_message_id: message.id,
    });
    await request("GET", `/agents/${agent}/sessions`);
    state.messagePicker.open = false;
    showCopyStatus("Creating branch; it activates when ready");
  } catch (error) {
    showCopyStatus(`Fork failed: ${error}`);
  } finally {
    state.forkBusy = false;
    render();
  }
}

function legacyOpenMessagePicker() {
  if (!selectableMessages().length) {
    showCopyStatus("No text messages available");
    return;
  }
  state.messagePicker = { open: true, mode: "copy", query: "", selected: 0 };
  render();
}

function closeMessagePicker() {
  state.messagePicker = { open: false, mode: state.messagePicker.mode, query: "", selected: 0 };
  render();
}

async function copyText(text: string) {
  try {
    if (navigator.clipboard?.writeText) {
      await navigator.clipboard.writeText(text);
    } else {
      throw new Error("Clipboard API unavailable");
    }
    showCopyStatus("Copied message");
  } catch (error) {
    const copied = legacyCopyText(text);
    showCopyStatus(copied ? "Copied message" : `Copy failed: ${error}`);
  }
}

function legacyCopyText(text: string): boolean {
  const area = document.createElement("textarea");
  area.value = text;
  area.style.position = "fixed";
  area.style.opacity = "0";
  document.body.append(area);
  area.select();
  const copied = document.execCommand("copy");
  area.remove();
  return copied;
}

function showCopyStatus(text: string) {
  state.copyStatus = text;
  let toast = document.querySelector<HTMLDivElement>("#copy-status");
  if (!toast) {
    toast = document.createElement("div");
    toast.id = "copy-status";
    document.body.append(toast);
  }
  toast.textContent = text;
  window.setTimeout(() => {
    if (state.copyStatus === text) {
      state.copyStatus = null;
      toast?.remove();
    }
  }, 2200);
}

function filteredMessages(): ChatItem[] {
  const needle = state.messagePicker.query.trim().toLowerCase();
  return selectableMessages().filter((item) =>
    !needle || `${item.role} ${messagePreview(item.text)} ${item.text}`.toLowerCase().includes(needle),
  );
}

function filteredForkMessages(): PickerForkItem[] {
  const needle = state.messagePicker.query.trim().toLowerCase();
  return forkableMessages().filter((item) =>
    !needle || `${item.role} ${messagePreview(item.text)} ${item.text}`.toLowerCase().includes(needle),
  );
}

function renderMessagePickerList(container: HTMLDivElement) {
  container.replaceChildren();
  const isFork = state.messagePicker.mode === "fork";
  const items: (ChatItem | PickerForkItem)[] = isFork ? filteredForkMessages() : filteredMessages();
  if (state.messagePicker.selected >= items.length) state.messagePicker.selected = Math.max(0, items.length - 1);
  if (!items.length) {
    container.innerHTML = `<div class="muted">No matching messages</div>`;
    return;
  }
  items.forEach((item, index) => {
    const row = document.createElement("button");
    row.type = "button";
    row.className = `picker-row ${index === state.messagePicker.selected ? "active" : ""}`;
    const role = item.role;
    const text = item.text;
    row.innerHTML = `
      <span class="picker-role ${role}">${role === "user" ? "YOU" : "AI"}</span>
      <span>${escapeHtml(messagePreview(text))}</span>`;
    row.onclick = () => {
      state.messagePicker.selected = index;
      if (isFork) void forkFromMessage(item as PickerForkItem);
      else void copyText(text);
      renderMessagePickerList(container);
    };
    container.append(row);
  });
}

function renderMessagePickerModal() {
  document.querySelector("#message-picker")?.remove();
  if (!state.messagePicker.open || state.view !== "agent") return;
  const overlay = document.createElement("div");
  overlay.id = "message-picker";
  const modal = document.createElement("div");
  modal.className = "picker-modal";
  const title = document.createElement("h3");
  title.textContent = state.messagePicker.mode === "fork" ? "Fork from message" : "Copy message";
  const input = document.createElement("input");
  input.value = state.messagePicker.query;
  input.placeholder = "Filter messages";
  input.autocomplete = "off";
  input.oninput = () => {
    state.messagePicker.query = input.value;
    state.messagePicker.selected = 0;
    renderMessagePickerList(list);
  };
  input.addEventListener("keydown", (event) => {
    const items = state.messagePicker.mode === "fork" ? filteredForkMessages() : filteredMessages();
    if (event.key === "Escape") {
      event.preventDefault();
      closeMessagePicker();
    } else if (event.key === "ArrowUp") {
      event.preventDefault();
      state.messagePicker.selected = Math.max(0, state.messagePicker.selected - 1);
      renderMessagePickerList(list);
    } else if (event.key === "ArrowDown") {
      event.preventDefault();
      state.messagePicker.selected = Math.min(items.length - 1, state.messagePicker.selected + 1);
      renderMessagePickerList(list);
    } else if (event.key === "Enter") {
      event.preventDefault();
      const item = items[state.messagePicker.selected];
      if (!item) return;
      if (state.messagePicker.mode === "fork") void forkFromMessage(item as PickerForkItem);
      else void copyText((item as ChatItem).text);
    }
  });
  const list = document.createElement("div");
  list.className = "picker-list";
  const help = document.createElement("footer");
  help.textContent = state.messagePicker.mode === "fork"
    ? "Click or Enter forks · Up/Down navigate · Esc closes"
    : "Click or Enter copies · Up/Down navigate · Esc closes";
  modal.append(title, input, list, help);
  overlay.append(modal);
  overlay.onclick = (event) => {
    if (event.target === overlay) closeMessagePicker();
  };
  document.body.append(overlay);
  renderMessagePickerList(list);
  input.focus();
  input.setSelectionRange(input.value.length, input.value.length);
}

function renderBibRequestsModal() {
  document.querySelector("#bib-requests")?.remove();
  if (!state.bibRequestsOpen) return;
  const overlay = document.createElement("div");
  overlay.id = "bib-requests";
  const modal = document.createElement("div");
  modal.className = "picker-modal";
  const title = document.createElement("h3");
  title.textContent = "Pending full-text requests";
  const list = document.createElement("div");
  list.className = "picker-list";
  const refresh = () => {
    list.replaceChildren();
    if (state.bibRequestsLoading) {
      list.innerHTML = `<div class="muted">Loading requests…</div>`;
      return;
    }
    if (!state.bibRequests.length) {
      list.innerHTML = `<div class="empty">No pending requests</div>`;
      return;
    }
    for (const request of state.bibRequests) {
      const row = document.createElement("article");
      row.className = "resume-row";
      row.innerHTML = `
        <div>
          <strong>${escapeHtml(request.article_id)}</strong>
          <small>${escapeHtml(request.collection_id)} · ${escapeHtml(request.role)} · ${escapeHtml(request.added_by)}</small>
          ${request.note ? `<small>${escapeHtml(request.note)}</small>` : ""}
        </div>`;
      const actions = document.createElement("div");
      actions.className = "resume-actions";
      const open = document.createElement("button");
      open.textContent = "Open";
      open.onclick = () => openBibRequest(request);
      actions.append(open);
      row.append(actions);
      list.append(row);
    }
  };
  const help = document.createElement("footer");
  help.textContent = "Open the article, then upload its full text";
  modal.append(title, list, help);
  overlay.append(modal);
  overlay.onclick = (event) => {
    if (event.target === overlay) closeBibRequests();
  };
  document.body.append(overlay);
  refresh();
}

function renderBibImportModal() {
  document.querySelector("#bib-import")?.remove();
  if (!state.bibImportOpen) return;
  const overlay = document.createElement("div");
  overlay.id = "bib-import";
  const modal = document.createElement("div");
  modal.className = "picker-modal";
  const title = document.createElement("h3");
  title.textContent = "Import article";
  const form = document.createElement("form");
  form.className = "model-form";
  const typeLabel = document.createElement("label");
  typeLabel.innerHTML = `<span>Identifier type</span>`;
  const type = document.createElement("select");
  for (const value of ["doi", "pmid", "pmc", "arxiv", "biorxiv", "s2", "openalex"]) {
    const option = document.createElement("option");
    option.value = value;
    option.textContent = value;
    option.selected = state.bibImport.idType === value;
    type.append(option);
  }
  type.onchange = () => {
    state.bibImport.idType = type.value;
  };
  typeLabel.append(type);
  const idLabel = document.createElement("label");
  idLabel.innerHTML = `<span>Identifier</span>`;
  const id = document.createElement("input");
  id.value = state.bibImport.id;
  id.required = true;
  id.placeholder = state.bibImport.idType === "doi" ? "10.1100/example" : "identifier";
  id.oninput = () => {
    state.bibImport.id = id.value;
  };
  idLabel.append(id);
  const sourceLabel = document.createElement("label");
  sourceLabel.innerHTML = `<span>Source override (optional)</span>`;
  const source = document.createElement("input");
  source.value = state.bibImport.source;
  source.placeholder = "pubmed, crossref, arxiv…";
  source.oninput = () => {
    state.bibImport.source = source.value;
  };
  sourceLabel.append(source);
  const fetchLabel = document.createElement("label");
  fetchLabel.className = "flag-grid";
  const fetch = document.createElement("input");
  fetch.type = "checkbox";
  fetch.checked = state.bibImport.fetchFulltext;
  fetch.onchange = () => {
    state.bibImport.fetchFulltext = fetch.checked;
  };
  fetchLabel.append(fetch, Object.assign(document.createElement("span"), { textContent: "Fetch full text" }));
  const actions = document.createElement("div");
  actions.className = "actions";
  const cancel = document.createElement("button");
  cancel.type = "button";
  cancel.textContent = "Cancel";
  cancel.onclick = closeBibImport;
  const save = document.createElement("button");
  save.type = "submit";
  save.textContent = state.bibImportBusy ? "Importing…" : "Import";
  save.disabled = state.bibImportBusy;
  actions.append(cancel, save);
  form.append(typeLabel, idLabel, sourceLabel, fetchLabel, actions);
  form.onsubmit = (event) => {
    event.preventDefault();
    void importBibArticle();
  };
  modal.append(title, form);
  overlay.append(modal);
  overlay.onclick = (event) => {
    if (event.target === overlay) closeBibImport();
  };
  document.body.append(overlay);
  id.focus();
}

function updateResumePickerList(container: HTMLDivElement) {
  container.replaceChildren();
  const agents = filteredStoredAgents();
  if (state.resumePicker.selected >= agents.length) {
    state.resumePicker.selected = Math.max(0, agents.length - 1);
  }
  if (state.resumePicker.loading) {
    container.innerHTML = `<div class="muted">Loading persisted agents…</div>`;
    return;
  }
  if (!agents.length) {
    container.innerHTML = `<div class="empty">No matching persisted agents</div>`;
    return;
  }
  for (const agent of agents.slice(0, 300)) {
    const running = state.agents.find((item) => item.agent_id === agent.id);
    const row = document.createElement("article");
    row.className = `resume-row ${state.resumePicker.selected === agents.indexOf(agent) ? "active" : ""}`;
    const kind = agent.config_json.kind ?? "researcher";
    const model = agent.config_json.preferred_model ?? "default model";
    row.innerHTML = `
      <div>
        <strong>${escapeHtml(agent.name)}</strong>
        <small>${escapeHtml(kind)} · ${escapeHtml(model)} · ${escapeHtml(new Date(agent.last_active).toLocaleString())}</small>
      </div>`;
    const actions = document.createElement("div");
    actions.className = "resume-actions";
    const open = document.createElement("button");
    open.textContent = running ? "Focus" : state.resumeBusy === agent.id ? "Resuming…" : "Resume";
    open.disabled = state.resumeBusy === agent.id;
    open.onclick = () => void resumeStoredAgent(agent);
    const rename = document.createElement("button");
    rename.textContent = "Rename";
    rename.disabled = Boolean(state.resumeBusy);
    rename.onclick = () => {
      state.renameAgent = { id: agent.id, name: leafName(agent.name) };
      state.deleteAgent = null;
      updateResumePickerList(container);
    };
    const remove = document.createElement("button");
    remove.textContent = "Delete";
    remove.disabled = Boolean(state.resumeBusy);
    remove.onclick = () => {
      state.deleteAgent = { id: agent.id, confirm: "" };
      state.renameAgent = null;
      updateResumePickerList(container);
    };
    actions.append(open, rename, remove);
    row.append(actions);
    if (state.renameAgent?.id === agent.id) {
      const form = document.createElement("form");
      form.className = "inline-session-form";
      const input = document.createElement("input");
      input.value = state.renameAgent.name;
      input.oninput = () => {
        if (state.renameAgent) state.renameAgent.name = input.value;
      };
      const save = document.createElement("button");
      save.textContent = "Save";
      const cancel = document.createElement("button");
      cancel.type = "button";
      cancel.textContent = "Cancel";
      cancel.onclick = () => {
        state.renameAgent = null;
        updateResumePickerList(container);
      };
      form.append(input, save, cancel);
      form.onsubmit = (event) => {
        event.preventDefault();
        state.renameAgent!.name = input.value;
        void renameStoredAgent().finally(() => {
          document.querySelector<HTMLDivElement>("#resume-list") && updateResumePickerList(container);
        });
      };
      row.append(form);
      if (state.renameAgent.error) {
        const error = document.createElement("small");
        error.className = "error-text";
        error.textContent = state.renameAgent.error;
        row.append(error);
      }
    }
    if (state.deleteAgent?.id === agent.id) {
      const form = document.createElement("form");
      form.className = "inline-session-form danger";
      const input = document.createElement("input");
      input.value = state.deleteAgent.confirm;
      input.placeholder = 'type "yes"';
      const button = document.createElement("button");
      button.textContent = "Delete forever";
      form.append(input, button);
      form.onsubmit = (event) => {
        event.preventDefault();
        state.deleteAgent!.confirm = input.value;
        void deleteStoredAgent().finally(() => {
          if (document.querySelector("#resume-list")) updateResumePickerList(container);
        });
      };
      row.append(form);
    }
    row.onclick = (event) => {
      if (event.target === row) {
        state.resumePicker.selected = agents.indexOf(agent);
        updateResumePickerList(container);
      }
    };
    container.append(row);
  }
  if (agents.length > 300) {
    const note = document.createElement("footer");
    note.textContent = `Showing newest 300 of ${agents.length.toLocaleString()} matches; refine the filter.`;
    container.append(note);
  }
}

function renderResumePickerModal() {
  document.querySelector("#resume-picker")?.remove();
  if (!state.resumePicker.open) return;
  const overlay = document.createElement("div");
  overlay.id = "resume-picker";
  const modal = document.createElement("div");
  modal.className = "picker-modal resume";
  const title = document.createElement("h3");
  title.textContent = "Resume persisted agent";
  const input = document.createElement("input");
  input.value = state.resumePicker.query;
  input.placeholder = "Filter by path or profile";
  input.oninput = () => {
    state.resumePicker.query = input.value;
    state.resumePicker.selected = 0;
    updateResumePickerList(list);
  };
  const list = document.createElement("div");
  list.id = "resume-list";
  list.className = "picker-list";
  const help = document.createElement("footer");
  help.textContent = "Click Resume to restore · Rename edits the stored path · Delete requires yes";
  modal.append(title, input, list, help);
  overlay.append(modal);
  overlay.onclick = (event) => {
    if (event.target === overlay) closeResumePicker();
  };
  document.body.append(overlay);
  updateResumePickerList(list);
  input.focus();
}

type GUICommand = {
  id: string;
  title: string;
  category: string;
  keywords: string;
  available?: () => boolean;
  run: () => void;
};

function commandActions(): GUICommand[] {
  const active = Boolean(state.activeAgent);
  return [
    {
      id: "new-agent",
      title: "New agent",
      category: "agent",
      keywords: "spawn create start profile",
      run: () => {
        state.view = "agent";
        state.spawnOpen = true;
        render();
      },
    },
    {
      id: "resume-agent",
      title: "Resume agent",
      category: "agent",
      keywords: "restore reconnect continue previous record",
      run: openResumePicker,
    },
    {
      id: "sessions",
      title: "Sessions",
      category: "agent",
      keywords: "conversation switch fork list branch",
      run: () => {
        state.view = "agent";
        render();
      },
    },
    {
      id: "new-session",
      title: "New session",
      category: "agent",
      keywords: "new conversation branch",
      available: () => active,
      run: () => {
        state.view = "agent";
        state.sessionCreating = true;
        state.sessionCreateTitle = "";
        render();
      },
    },
    {
      id: "close-agent",
      title: "Close agent",
      category: "agent",
      keywords: "shutdown terminate stop close leaf",
      available: () => active,
      run: () => void shutdownAgent(state.activeAgent),
    },
    {
      id: "delete-agent",
      title: "Delete agent",
      category: "agent",
      keywords: "delete remove purge permanent storage erase",
      available: () => active,
      run: () => void deleteActiveAgent(),
    },
    {
      id: "model-config",
      title: "Select model",
      category: "config",
      keywords: "model provider api key activate",
      run: () => {
        state.view = "config";
        render();
      },
    },
    {
      id: "embedding",
      title: "Embedding model",
      category: "config",
      keywords: "embedding ollama vector provider",
      run: () => {
        state.view = "config";
        render();
      },
    },
    {
      id: "agent-config",
      title: "Agent settings",
      category: "agent",
      keywords: "memory runtime reasoning effort",
      available: () => active,
      run: () => {
        state.view = "config";
        render();
      },
    },
    {
      id: "skills",
      title: "Evolution dashboard",
      category: "skills",
      keywords: "kms evolution proposals distill governance",
      run: () => {
        state.view = "kms";
        state.kmsTab = "skills";
        render();
      },
    },
    {
      id: "wiki",
      title: "Knowledge wiki",
      category: "kms",
      keywords: "knowledge wiki tree nodes",
      run: () => {
        state.view = "kms";
        state.kmsTab = "wiki";
        render();
      },
    },
    {
      id: "proposals",
      title: "Review KMS proposals",
      category: "skills",
      keywords: "approve reject pending proposals",
      run: () => {
        state.view = "kms";
        state.kmsTab = "skills";
        render();
      },
    },
    {
      id: "dag",
      title: "View DAG",
      category: "data",
      keywords: "graph pipeline topology node edge",
      run: () => {
        state.view = "dag";
        render();
      },
    },
    {
      id: "bibliography",
      title: "Bibliography library",
      category: "research",
      keywords: "papers articles fulltext collections",
      run: () => {
        state.view = "bib";
        render();
      },
    },
    {
      id: "history-search",
      title: "Search history",
      category: "agent",
      keywords: "ctrl r recall previous prompt",
      available: () => active && promptHistory(state.activeAgent).length > 0,
      run: () => {
        state.view = "agent";
        render();
        const composer = document.querySelector<HTMLTextAreaElement>("#composer");
        if (composer) startPromptSearch(state.activeAgent, composer);
      },
    },
    {
      id: "cancel",
      title: "Cancel agent",
      category: "agent",
      keywords: "stop abort interrupt",
      available: () => active,
      run: cancelAgent,
    },
    {
      id: "compact",
      title: "Compact conversation",
      category: "agent",
      keywords: "compress summarize context shrink",
      available: () => active,
      run: compactAgent,
    },
    {
      id: "scroll-bottom",
      title: "Scroll to bottom",
      category: "view",
      keywords: "tail latest end follow",
      available: () => active,
      run: () => {
        state.autoScroll.set(state.activeAgent, true);
        document.querySelector<HTMLDivElement>("#transcript")?.scrollTo(0, 999999999);
      },
    },
    {
      id: "scroll-top",
      title: "Scroll to top",
      category: "view",
      keywords: "home beginning first",
      available: () => active,
      run: () => {
        state.autoScroll.set(state.activeAgent, false);
        document.querySelector<HTMLDivElement>("#transcript")?.scrollTo(0, 0);
      },
    },
    {
      id: "toggle-autoscroll",
      title: "Toggle auto-scroll",
      category: "view",
      keywords: "follow pin lock ctrl g",
      available: () => active,
      run: () => {
        const next = !(state.autoScroll.get(state.activeAgent) ?? true);
        state.autoScroll.set(state.activeAgent, next);
        if (next) document.querySelector<HTMLDivElement>("#transcript")?.scrollTo(0, 999999999);
      },
    },
    {
      id: "thinking",
      title: "Toggle thinking blocks",
      category: "view",
      keywords: "collapse fold reasoning",
      run: () => void saveDisplaySetting("collapse_thinking", !state.display.collapse_thinking),
    },
    {
      id: "tool-calls",
      title: "Toggle tool calls",
      category: "view",
      keywords: "collapse fold function call",
      run: () => void saveDisplaySetting("collapse_tool_calls", !state.display.collapse_tool_calls),
    },
    {
      id: "tool-results",
      title: "Toggle tool results",
      category: "view",
      keywords: "collapse fold output response",
      run: () => void saveDisplaySetting("collapse_tool_results", !state.display.collapse_tool_results),
    },
    {
      id: "copy-message",
      title: "Copy message",
      category: "clipboard",
      keywords: "clipboard transcript select text",
      available: () => active,
      run: legacyOpenMessagePicker,
    },
    {
      id: "fork-message",
      title: "Fork from message",
      category: "session",
      keywords: "branch conversation message history",
      available: () => active && Boolean(state.activeSessions.get(state.activeAgent)),
      run: () => void openForkMessagePicker(),
    },
    {
      id: "reload-config",
      title: "Reload config",
      category: "config",
      keywords: "refresh providers model catalogue",
      run: () => void refreshModelCatalog().then(render),
    },
  ];
}

function filteredCommands(): GUICommand[] {
  const needle = state.commandPalette.query.trim().toLowerCase();
  return commandActions().filter((command) => {
    if (command.available && !command.available()) return false;
    return !needle || `${command.title} ${command.category} ${command.keywords}`.toLowerCase().includes(needle);
  });
}

function openCommandPalette() {
  state.commandPalette = { open: true, query: "", selected: 0 };
  render();
}

function closeCommandPalette() {
  state.commandPalette = { open: false, query: "", selected: 0 };
  render();
}

function runCommand(command: GUICommand) {
  state.commandPalette = { open: false, query: "", selected: 0 };
  command.run();
}

function renderCommandPaletteModal() {
  document.querySelector("#command-palette")?.remove();
  if (!state.commandPalette.open) return;
  const overlay = document.createElement("div");
  overlay.id = "command-palette";
  const modal = document.createElement("div");
  modal.className = "picker-modal command";
  const title = document.createElement("h3");
  title.textContent = "Command palette";
  const input = document.createElement("input");
  input.value = state.commandPalette.query;
  input.placeholder = "Search commands";
  input.autocomplete = "off";
  const list = document.createElement("div");
  list.className = "picker-list";
  const updateList = () => {
    const filtered = filteredCommands();
    list.replaceChildren();
    if (!filtered.length) {
      list.innerHTML = `<div class="muted">No matching commands</div>`;
      return;
    }
    filtered.forEach((command, index) => {
      const row = document.createElement("button");
      row.type = "button";
      row.className = `picker-row ${index === state.commandPalette.selected ? "active" : ""}`;
      row.innerHTML = `
        <span>${escapeHtml(command.title)}</span>
        <small>${escapeHtml(command.category)}</small>`;
      row.onclick = () => runCommand(command);
      list.append(row);
    });
  };
  input.oninput = () => {
    state.commandPalette.query = input.value;
    state.commandPalette.selected = 0;
    updateList();
  };
  input.addEventListener("keydown", (event) => {
    const filtered = filteredCommands();
    if (event.key === "Escape") {
      event.preventDefault();
      closeCommandPalette();
    } else if (event.key === "ArrowUp") {
      event.preventDefault();
      state.commandPalette.selected = Math.max(0, state.commandPalette.selected - 1);
      updateList();
    } else if (event.key === "ArrowDown") {
      event.preventDefault();
      state.commandPalette.selected = Math.min(filtered.length - 1, state.commandPalette.selected + 1);
      updateList();
    } else if (event.key === "Enter") {
      event.preventDefault();
      const command = filtered[state.commandPalette.selected];
      if (command) runCommand(command);
    }
  });
  const help = document.createElement("footer");
  help.textContent = "Ctrl+P opens · type to filter · Enter runs · Esc closes";
  modal.append(title, input, list, help);
  overlay.append(modal);
  overlay.onclick = (event) => {
    if (event.target === overlay) closeCommandPalette();
  };
  document.body.append(overlay);
  updateList();
  input.focus();
}

function renderAgentConfigCard(container: HTMLDivElement) {
  const card = document.createElement("section");
  card.className = "provider-card agent-config";
  card.innerHTML = `
    <div class="provider-header static">
      <strong>Agent Runtime</strong>
      <span>${escapeHtml(state.activeAgent || "no agent")}</span>
    </div>`;
  if (!state.activeAgent) {
    const empty = document.createElement("p");
    empty.className = "muted";
    empty.textContent = "选择一个 agent 后即可编辑 runtime overrides。";
    card.append(empty);
    container.append(card);
    return;
  }
  if (state.agentConfigLoading) {
    const loading = document.createElement("p");
    loading.className = "muted";
    loading.textContent = "Loading…";
    card.append(loading);
    container.append(card);
    return;
  }
  if (state.agentConfigError) {
    const error = document.createElement("p");
    error.className = "muted error-text";
    error.textContent = state.agentConfigError;
    card.append(error);
  }

  const rows = document.createElement("div");
  rows.className = "runtime-rows";
  const controls: Array<{
    label: string;
    key: "use_memory" | "generate_memory";
    effective?: boolean;
  }> = [
    {
      label: "Memory injection",
      key: "use_memory",
      effective: state.agentEffective?.use_memory,
    },
    {
      label: "Memory generation",
      key: "generate_memory",
      effective: state.agentEffective?.generate_memory,
    },
  ];
  for (const control of controls) {
    const row = document.createElement("div");
    row.className = "runtime-row";
    const label = document.createElement("label");
    label.textContent = control.label;
    const segmented = document.createElement("div");
    segmented.className = "segmented compact";
    const value = state.agentRuntime[control.key];
    for (const option of [
      ["inherit", undefined] as const,
      ["on", true] as const,
      ["off", false] as const,
    ]) {
      const button = document.createElement("button");
      button.textContent = option[0];
      button.className =
        (value === undefined || value === null ? option[1] === undefined : option[1] === value)
          ? "active"
          : "";
      button.onclick = () => {
        state.agentRuntime[control.key] = option[1];
        render();
      };
      segmented.append(button);
    }
    const effective = document.createElement("small");
    effective.textContent = control.effective === undefined ? "" : `effective ${control.effective ? "on" : "off"}`;
    row.append(label, segmented, effective);
    rows.append(row);
  }

  const effortRow = document.createElement("div");
  effortRow.className = "runtime-row";
  const effortLabel = document.createElement("label");
  effortLabel.textContent = "Reasoning effort";
  const effortSelect = document.createElement("select");
  for (const option of [
    ["inherit", ""],
    ["none", "none"],
    ["minimal", "minimal"],
    ["low", "low"],
    ["medium", "medium"],
    ["high", "high"],
    ["xhigh", "xhigh"],
    ["max", "max"],
    ["ultra", "ultra"],
  ] as const) {
    const element = document.createElement("option");
    element.value = option[1];
    element.textContent = option[0];
    element.selected = option[1] === "" ? !state.agentRuntime.reasoning_effort : state.agentRuntime.reasoning_effort === option[1];
    effortSelect.append(element);
  }
  effortSelect.onchange = () => {
    state.agentRuntime.reasoning_effort = (effortSelect.value || undefined) as AgentRuntimeOverrides["reasoning_effort"];
  };
  const effortEffective = document.createElement("small");
  effortEffective.textContent = state.agentEffective?.reasoning_effort ? `effective ${state.agentEffective.reasoning_effort}` : "effective model default";
  effortRow.append(effortLabel, effortSelect, effortEffective);
  rows.append(effortRow);
  card.append(rows);

  const actions = document.createElement("div");
  actions.className = "actions";
  const reset = document.createElement("button");
  reset.textContent = "All inherit";
  reset.onclick = () => {
    state.agentRuntime = {};
    render();
  };
  const save = document.createElement("button");
  save.textContent = state.agentConfigSaving ? "Saving…" : "Save";
  save.disabled = state.agentConfigSaving;
  save.onclick = () => void saveAgentConfig();
  actions.append(reset, save);
  card.append(actions);
  container.append(card);
}

function renderConfig(container: HTMLDivElement) {
  container.replaceChildren();
  if (state.configStatus) {
    const status = document.createElement("div");
    status.className = `config-status ${state.configStatus.kind}`;
    if (state.configStatus.text.startsWith("Authorize in the browser: ")) {
      const prefix = "Authorize in the browser: ";
      const url = state.configStatus.text.slice(prefix.length);
      status.textContent = `${prefix}`;
      const link = document.createElement("a");
      link.href = url;
      link.textContent = url;
      link.target = "_blank";
      link.rel = "noopener";
      status.append(link);
    } else {
      status.textContent = state.configStatus.text;
    }
    container.append(status);
  }
  renderAgentConfigCard(container);
  renderDisplaySettingsCard(container);
  renderEmbeddingCard(container);
  for (const provider of state.catalog.providers) {
    const card = document.createElement("section");
    card.className = "provider-card";
    const expanded = state.expanded.has(provider.name) || state.selectedProvider === provider.name;
    const header = document.createElement("button");
    header.type = "button";
    header.className = "provider-header";
    const configured = Boolean(provider.api_key);
    header.innerHTML = `<span class="disclosure">${expanded ? "▾" : "▸"}</span><strong>${escapeHtml(provider.name)}</strong><span>${configured ? "已配置" : "未配置"}</span>`;
    header.onclick = () => {
      if (state.expanded.has(provider.name)) state.expanded.delete(provider.name);
      else state.expanded.add(provider.name);
      render();
    };
    card.append(header);
    if (expanded) {
      const models = providerModels(provider.name);
      for (const model of models) {
        const spec = `${provider.name}:${model.model_name}`;
        const row = document.createElement("div");
        row.className = "model-row";
        row.innerHTML = `<span class="dot">${state.activeModel === spec ? "●" : ""}</span><span>${escapeHtml(model.model_name)}</span><small>${escapeHtml(model.context_length.toLocaleString())} ctx</small>`;
        const activate = document.createElement("button");
        activate.type = "button";
        activate.textContent = state.activeModel === spec ? "active" : "activate";
        activate.disabled = state.activeModel === spec;
        activate.onclick = () => setDefaultModel(spec);
        row.append(activate);
        if (model.custom) {
          const edit = document.createElement("button");
          edit.type = "button";
          edit.textContent = "edit";
          edit.onclick = () => openEditModel(provider.name, model.model_name);
          const remove = document.createElement("button");
          remove.type = "button";
          const pendingDelete = state.confirmDelete === spec;
          remove.textContent = pendingDelete ? "confirm" : "delete";
          remove.className = pendingDelete ? "danger" : "";
          remove.onclick = () => void deleteCustomModel(provider.name, model.model_name);
          row.append(edit, remove);
        }
        card.append(row);
      }
      if (!models.length) {
        const empty = document.createElement("p");
        empty.className = "muted";
        empty.textContent = "暂无模型";
        card.append(empty);
      }
      const keyButton = document.createElement("button");
      keyButton.type = "button";
      keyButton.className = "ghost";
      keyButton.textContent = provider.api_key ? "编辑 API Key" : "添加 API Key";
      keyButton.onclick = () => {
        state.selectedProvider = provider.name;
        state.apiKey = provider.api_key;
        state.providerBaseUrl = provider.base_url;
        render();
      };
      if (provider.provider_type !== "openai") card.append(keyButton);
      const controls = document.createElement("div");
      controls.className = "provider-actions";
      const add = document.createElement("button");
      add.type = "button";
      add.textContent = state.modelForm?.provider_name === provider.name && !state.modelForm.original_model_name
        ? "Cancel add"
        : "Add model";
      add.onclick = () => {
        if (add.textContent === "Cancel add") {
          state.modelForm = null;
          render();
        } else {
          openAddModel(provider.name);
        }
      };
      controls.append(add);
      if (supportsRemoteCatalog(provider.provider_type)) {
        const fetch = document.createElement("button");
        fetch.type = "button";
        fetch.textContent = state.catalogBusy === provider.name ? "Fetching…" : "Fetch catalog";
        fetch.disabled = state.catalogBusy === provider.name;
        fetch.onclick = () => void fetchRemoteCatalog(provider.name, provider.provider_type, provider.base_url);
        controls.append(fetch);
      }
      if (provider.provider_type === "openai") {
        const token = state.catalog.openai_token;
        const status = document.createElement("p");
        status.className = "muted";
        status.textContent = !token?.present
          ? "ChatGPT subscription: not signed in"
          : token.expired
            ? "ChatGPT subscription: token expired"
            : "ChatGPT subscription: token active";
        card.append(status);
        const login = document.createElement("button");
        login.type = "button";
        login.textContent = "ChatGPT login";
        login.onclick = () => void startChatgptLogin();
        const refresh = document.createElement("button");
        refresh.type = "button";
        refresh.textContent = "Refresh token";
        refresh.onclick = () => void refreshChatgpt();
        controls.append(login, refresh);
      }
      card.append(controls);
      if (state.selectedProvider === provider.name) {
        const keyForm = document.createElement("form");
        keyForm.className = "key-form";
        const input = document.createElement("input");
        input.type = "password";
        input.value = state.apiKey;
        input.placeholder = "API Key";
        input.oninput = () => {
          state.apiKey = input.value;
        };
        const baseUrlLabel = document.createElement("label");
        baseUrlLabel.className = "key-field";
        baseUrlLabel.innerHTML = `<span>Base URL</span>`;
        const baseUrl = document.createElement("input");
        baseUrl.type = "text";
        baseUrl.value = state.providerBaseUrl;
        baseUrl.placeholder = "https://provider.example/v1";
        baseUrl.oninput = () => {
          state.providerBaseUrl = baseUrl.value;
        };
        baseUrlLabel.append(baseUrl);
        const buttons = document.createElement("div");
        buttons.className = "key-buttons";
        const save = document.createElement("button");
        save.type = "submit";
        save.textContent = "保存";
        const cancel = document.createElement("button");
        cancel.type = "button";
        cancel.textContent = "取消";
        cancel.onclick = () => {
          state.selectedProvider = "";
          state.apiKey = "";
          state.providerBaseUrl = "";
          render();
        };
        buttons.append(save, cancel);
        keyForm.append(input, baseUrlLabel, buttons);
        keyForm.onsubmit = (event) => {
          event.preventDefault();
          saveProviderKey(provider.name, state.apiKey, state.providerBaseUrl);
        };
        card.append(keyForm);
      }
      if (state.modelForm?.provider_name === provider.name) {
        const form = document.createElement("form");
        form.className = "model-form";
        const title = document.createElement("h4");
        title.textContent = state.modelForm.original_model_name
          ? `Edit ${state.modelForm.original_model_name}`
          : "Add custom model";
        form.append(title);
        const textFields: Array<[keyof CustomModelDraft, string, "text" | "number"]> = [
          ["model_name", "Model name", "text"],
          ["context_length", "Context length", "number"],
          ["max_output_tokens", "Max output", "number"],
          ["input_token_price", "Input price", "number"],
          ["output_token_price", "Output price", "number"],
        ];
        for (const [key, label, type] of textFields) {
          const field = document.createElement("label");
          field.innerHTML = `<span>${escapeHtml(label)}</span>`;
          const input = document.createElement("input");
          input.type = type;
          input.value = String(state.modelForm[key] ?? "");
          input.required = key === "model_name";
          if (type === "number") {
            input.min = "0";
            input.step = "any";
          }
          input.oninput = () => {
            if (state.modelForm) state.modelForm[key] = input.value as never;
          };
          field.append(input);
          form.append(field);
        }
        const flags: Array<[keyof CustomModelDraft, string]> = [
          ["vision_ability", "Vision"],
          ["supports_function_calling", "Function calling"],
          ["supports_streaming", "Streaming"],
          ["supports_thinking", "Thinking support"],
          ["thinking_enabled", "Thinking enabled"],
        ];
        const flagGrid = document.createElement("div");
        flagGrid.className = "flag-grid";
        for (const [key, label] of flags) {
          const labelElement = document.createElement("label");
          const input = document.createElement("input");
          input.type = "checkbox";
          input.checked = Boolean(state.modelForm[key]);
          input.onchange = () => {
            if (state.modelForm) state.modelForm[key] = input.checked as never;
          };
          labelElement.append(input, Object.assign(document.createElement("span"), { textContent: label }));
          flagGrid.append(labelElement);
        }
        form.append(flagGrid);
        const actions = document.createElement("div");
        actions.className = "actions";
        const cancel = document.createElement("button");
        cancel.type = "button";
        cancel.textContent = "Cancel";
        cancel.onclick = () => {
          state.modelForm = null;
          state.confirmDelete = null;
          render();
        };
        const save = document.createElement("button");
        save.type = "submit";
        save.textContent = "Save model";
        actions.append(cancel, save);
        form.append(actions);
        form.onsubmit = (event) => {
          event.preventDefault();
          void saveCustomModel();
        };
        card.append(form);
      }
    }
    container.append(card);
  }
}

function renderDisplaySettingsCard(container: HTMLDivElement) {
  const card = document.createElement("section");
  card.className = "provider-card";
  card.innerHTML = `<div class="provider-header static"><strong>Display</strong><span>daemon settings</span></div>`;
  const rows = document.createElement("div");
  rows.className = "setting-rows";
  const entries: Array<[keyof typeof state.display, string]> = [
    ["collapse_thinking", "Collapse thinking"],
    ["collapse_tool_calls", "Collapse tool calls"],
    ["collapse_tool_results", "Collapse tool results"],
  ];
  for (const [key, label] of entries) {
    const row = document.createElement("label");
    row.className = "setting-row";
    const input = document.createElement("input");
    input.type = "checkbox";
    input.checked = state.display[key];
    input.onchange = () => void saveDisplaySetting(key, input.checked);
    const text = document.createElement("span");
    text.textContent = label;
    row.append(input, text);
    rows.append(row);
  }
  card.append(rows);
  container.append(card);
}

function renderEmbeddingCard(container: HTMLDivElement) {
  const card = document.createElement("section");
  card.className = "provider-card";
  card.innerHTML = `
    <div class="provider-header static">
      <strong>Embedding Model</strong>
      <span>${state.embeddingLoading ? "loading" : state.embedding.enabled ? "enabled" : "disabled"}</span>
    </div>`;
  if (state.embeddingLoading) {
    card.insertAdjacentHTML("beforeend", `<p class="muted">Loading…</p>`);
    container.append(card);
    return;
  }
  const form = document.createElement("form");
  form.className = "embedding-form";
  const enabledLabel = document.createElement("label");
  enabledLabel.className = "setting-row";
  const enabled = document.createElement("input");
  enabled.type = "checkbox";
  enabled.checked = state.embedding.enabled;
  enabled.onchange = () => {
    state.embedding.enabled = enabled.checked;
    render();
  };
  enabledLabel.append(enabled, Object.assign(document.createElement("span"), { textContent: "Enabled" }));
  form.append(enabledLabel);

  const fields: Array<["base_url" | "model", string, string, "text"] | ["batch_size" | "timeout_secs", string, string, "number"]> = [
    ["base_url", "Base URL", state.embedding.base_url, "text"],
    ["model", "Model", state.embedding.model, "text"],
    ["batch_size", "Batch size", String(state.embedding.batch_size), "number"],
    ["timeout_secs", "Timeout seconds", String(state.embedding.timeout_secs), "number"],
  ];
  for (const [key, label, value, type] of fields) {
    const row = document.createElement("label");
    row.className = "embedding-field";
    row.innerHTML = `<span>${escapeHtml(label)}</span>`;
    const input = document.createElement("input");
    input.type = type;
    input.value = value;
    input.required = true;
    if (type === "number") input.min = "1";
    input.onchange = () => {
      if (type === "number") {
        const parsed = Number(input.value);
        if (Number.isFinite(parsed) && parsed > 0) state.embedding[key] = parsed;
      } else {
        state.embedding[key] = input.value.trim() as string;
      }
    };
    row.append(input);
    form.append(row);
  }
  if (state.embeddingError) {
    form.insertAdjacentHTML("beforeend", `<p class="muted error-text">${escapeHtml(state.embeddingError)}</p>`);
  }
  const actions = document.createElement("div");
  actions.className = "actions";
  const reload = document.createElement("button");
  reload.type = "button";
  reload.textContent = "Reload";
  reload.onclick = () => void loadEmbeddingConfig(true);
  const save = document.createElement("button");
  save.type = "submit";
  save.textContent = state.embeddingSaving ? "Saving…" : "Save";
  save.disabled = state.embeddingSaving;
  actions.append(reload, save);
  form.append(actions);
  form.onsubmit = (event) => {
    event.preventDefault();
    void saveEmbeddingConfig();
  };
  card.append(form);
  container.append(card);
}

function renderOffline(message: string): void {
  app.innerHTML = "";
  const shell = document.createElement("main");
  shell.className = "offline";
  shell.innerHTML = `
    <section>
      <h1>Gateway 未运行</h1>
      <p>GUI 与 TUI 一样是薄客户端，不会启动第二个 runtime。</p>
      <pre>${escapeHtml(message)}</pre>
      <code>autonomics serve</code>
    </section>`;
  const retry = shell.querySelector("section")!;
  const button = document.createElement("button");
  button.textContent = "重试";
  button.onclick = () => void hydrate();
  retry.append(button);
  app.append(shell);
  reportFrontendReady("offline", {
    message,
    connected: false,
  });
  lastFrontendReadyStage = "offline";
}

function formatUnixTime(value: number | null | undefined): string {
  if (!value) return "—";
  return new Date(value < 10_000_000_000 ? value * 1000 : value).toLocaleString();
}

function renderKmsSummary(): string {
  const pending = state.kmsProposals.filter((item) => item.status === "pending").length;
  const approved = state.kmsProposals.filter((item) => item.status === "approved").length;
  const auto = state.kmsRows.filter((item) => item.tags.includes("auto")).length;
  const global = state.kmsRows.filter((item) => !["builtin", "workspace"].includes(item.tier)).length;
  const workspace = state.kmsRows.filter((item) => item.tier === "workspace").length;
  const builtin = state.kmsRows.filter((item) => item.tier === "builtin").length;
  const running = state.kmsWorkers.filter((worker) => worker.running).length;
  const proposer = state.kmsWorkers.find((worker) => worker.worker === "propose_skill_revision");
  return [
    `observations ${state.kmsEvidence.length}`,
    `skills ${auto} auto / ${global} global / ${workspace} workspace / ${builtin} builtin`,
    `proposals ${approved} approved · ${pending} pending`,
    `workers ${running}/${state.kmsWorkers.length} running`,
    `last cycle ${formatUnixTime(proposer?.last_run_at ?? null)}`,
  ].join("    ");
}

function renderKms(container: HTMLDivElement) {
  container.className = "transcript kms";
  container.replaceChildren();
  const summary = document.createElement("div");
  summary.className = "kms-summary";
  summary.textContent = renderKmsSummary();
  if (state.kmsLastReport) {
    const report = document.createElement("div");
    report.className = "kms-report";
    report.textContent = `last decision: ${state.kmsLastReport}`;
    summary.append(report);
  }

  const tabs = document.createElement("div");
  tabs.className = "segmented";
  for (const tab of [
    ["skills", `Skills (${state.kmsRows.length})`],
    ["wiki", `Wiki (${state.kmsKnowledge.length})`],
    ["evidence", `Evidence (${state.kmsEvidence.length})`],
  ] as const) {
    const button = document.createElement("button");
    button.textContent = tab[1];
    button.className = state.kmsTab === tab[0] ? "active" : "";
    button.onclick = () => {
      state.kmsTab = tab[0];
      render();
    };
    tabs.append(button);
  }
  const refresh = document.createElement("button");
  refresh.textContent = "Refresh";
  refresh.onclick = () => void loadKms(true);
  tabs.append(refresh);

  const content = document.createElement("div");
  content.className = "kms-content";
  if (state.kmsTab === "skills") {
    const rows = document.createElement("div");
    rows.className = "browser-list";
    for (const row of state.kmsRows) {
      const button = document.createElement("button");
      button.className = `browser-row ${state.kmsSelectedSkill === row.name ? "active" : ""}`;
      const status = row.proposal_status ? ` · ${row.proposal_status}${row.proposal_update ? "↑" : ""}` : "";
      button.innerHTML = `<strong>${escapeHtml(row.name)}</strong><small>[${escapeHtml(row.tier[0])}]${escapeHtml(status)}</small>`;
      button.onclick = () => {
        state.kmsSelectedSkill = row.name;
        void loadKmsSkillDetail(row.name);
        render();
      };
      rows.append(button);
    }
    if (!state.kmsRows.length) rows.innerHTML = `<div class="empty">No governed skills</div>`;

    const detail = document.createElement("article");
    detail.className = "browser-detail";
    const row = state.kmsRows.find((item) => item.name === state.kmsSelectedSkill);
    const skillDetail = state.kmsSkillDetail?.name === row?.name ? state.kmsSkillDetail : null;
    const heading = document.createElement("h3");
    heading.textContent = row ? row.name : "No skill selected";
    const meta = document.createElement("pre");
    if (row) {
      meta.textContent = [
        `Tier: ${row.tier}${row.installed ? " · installed" : " · not installed"}`,
        `Version: ${skillDetail ? skillDetail.installed ? "installed" : "proposed" : "—"}`,
        `Tags: ${row.tags.join(", ") || "—"}`,
        `Summary: ${row.description || "—"}`,
        `Proposal: ${row.proposal_status ? `${row.proposal_status}${row.proposal_update ? " update" : ""}` : "—"}`,
        `Usage: ${skillDetail ? `${skillDetail.usage_gets} get · ${skillDetail.usage_search_hits} search · ${skillDetail.usage_runs} run · ${skillDetail.usage_evals} eval` : "—"}`,
        `Evidence: ${skillDetail?.evidence_count ?? row.proposal?.source_evidence_ids.length ?? 0}`,
      ].join("\n");
    } else if (state.kmsSkillDetailLoading) {
      meta.textContent = "Loading skill detail…";
    } else {
      meta.textContent = "Select a skill to inspect.";
    }
    const body = document.createElement("div");
    const proposalBody = row?.proposal?.body_md;
    if (proposalBody || skillDetail?.body) {
      body.className = "markdown bordered";
      body.innerHTML = renderMarkdown(skillDetail?.body || proposalBody || "");
    }
    detail.append(heading, meta, body);
    if (row?.proposal_status === "pending" && row.proposal) {
      const actions = document.createElement("div");
      actions.className = "actions";
      const approve = document.createElement("button");
      approve.textContent = state.kmsActionBusy ? "Working…" : "Approve";
      approve.disabled = state.kmsActionBusy;
      approve.onclick = () => void decideKmsProposal(true);
      const reject = document.createElement("button");
      reject.textContent = "Reject";
      reject.disabled = state.kmsActionBusy;
      reject.onclick = () => void decideKmsProposal(false);
      actions.append(approve, reject);
      detail.append(actions);
    } else if (row?.proposal_status === "pending") {
      const unavailable = document.createElement("div");
      unavailable.className = "kms-report";
      unavailable.textContent =
        "Pending proposal detected, but this daemon does not expose proposal IDs. Restart the gateway with the newer KMS endpoints to review it.";
      detail.append(unavailable);
    }
    content.append(rows, detail);
  } else if (state.kmsTab === "wiki") {
    const rows = document.createElement("div");
    rows.className = "browser-list";
    for (const node of state.kmsKnowledge) {
      const depth = node.path.split("/").filter(Boolean).length - 1;
      const button = document.createElement("button");
      button.className = `browser-row ${state.kmsSelectedKnowledge === node.id ? "active" : ""}`;
      button.innerHTML = `<strong>${escapeHtml(`${"  ".repeat(Math.max(0, depth))}${node.title}`)}</strong><small>${escapeHtml(node.path)}</small>`;
      button.onclick = () => {
        state.kmsSelectedKnowledge = node.id;
        render();
      };
      rows.append(button);
    }
    if (!state.kmsKnowledge.length) rows.innerHTML = `<div class="empty">No wiki nodes</div>`;
    const detail = document.createElement("article");
    detail.className = "browser-detail";
    const node = state.kmsKnowledge.find((item) => item.id === state.kmsSelectedKnowledge);
    if (node) {
      const heading = document.createElement("h3");
      heading.textContent = node.title;
      const meta = document.createElement("pre");
      meta.textContent = `${node.path}\n${node.structural ? "section" : "article"} · ${node.status} · confidence ${node.confidence.toFixed(2)}\ncreated ${formatUnixTime(node.created_at)} · updated ${formatUnixTime(node.updated_at)}`;
      const body = document.createElement("div");
      body.className = "markdown bordered";
      body.innerHTML = renderMarkdown(node.body_md || "_empty body_");
      detail.append(heading, meta, body);
    } else {
      detail.innerHTML = `<div class="empty">No node selected</div>`;
    }
    content.append(rows, detail);
  } else {
    const rows = document.createElement("div");
    rows.className = "browser-list";
    for (const item of state.kmsEvidence) {
      const button = document.createElement("button");
      button.className = `browser-row ${state.kmsSelectedEvidence === item.id ? "active" : ""}`;
      button.innerHTML = `
        <strong>${escapeHtml(item.summary)}</strong>
        <small>${escapeHtml(`${item.status} · ${item.evidence_kind} · ${formatUnixTime(item.created_at)}`)}</small>`;
      button.onclick = () => {
        state.kmsSelectedEvidence = item.id;
        render();
      };
      rows.append(button);
    }
    if (!state.kmsEvidence.length) rows.innerHTML = `<div class="empty">No evidence recorded</div>`;
    const detail = document.createElement("article");
    detail.className = "browser-detail";
    const item = state.kmsEvidence.find((row) => row.id === state.kmsSelectedEvidence);
    if (item) {
      const heading = document.createElement("h3");
      heading.textContent = item.summary;
      const meta = document.createElement("pre");
      meta.textContent = [
        `ID: ${item.id}`,
        `Kind: ${item.evidence_kind}`,
        `Source: ${item.source_kind} · ${item.source_id}`,
        `Seen: ${formatUnixTime(item.last_seen_at)} ×${item.occurrence_count}`,
        item.anchor_path ? `Anchor: ${item.anchor_path}` : "",
        item.error ? `Error: ${item.error}` : "",
      ].filter(Boolean).join("\n");
      const body = document.createElement("pre");
      body.className = "bordered";
      body.textContent = item.body || "(empty body)";
      detail.append(heading, meta, body);
    } else {
      detail.innerHTML = `<div class="empty">No evidence selected</div>`;
    }
    content.append(rows, detail);
  }

  if (state.kmsLoading) {
    const loading = document.createElement("div");
    loading.className = "bib-status";
    loading.textContent = "Loading KMS…";
    container.append(loading);
  }
  if (state.kmsError) {
    const error = document.createElement("div");
    error.className = "bib-status error";
    error.textContent = state.kmsError;
    container.append(error);
  }
  container.append(summary, tabs, content);
}

function render(): void {
  if (!state.connected) return;
  app.innerHTML = "";
  const shell = document.createElement("main");
  shell.className = "shell";
  shell.innerHTML = `
    <header>
      <nav>
        <button data-view="agent">Agent</button>
        <button data-view="config">Config</button>
        <button data-view="bib">Bibliography</button>
        <button data-view="kms">KMS</button>
        <button data-view="dag">DAG</button>
      </nav>
      <div id="connection"></div>
    </header>
    <aside>
      <div class="section-title"><span>Agents</span><span><button id="resume-agent" class="mini">Resume</button><button id="new-agent" class="mini">New</button></span></div>
      <div id="agents"></div>
      <div id="spawn-form"></div>
      <div class="section-title"><span>Sessions</span><button id="new-session" class="mini">New</button></div>
      <div id="session-create"></div>
      <div id="sessions"></div>
    </aside>
    <section class="workspace">
      <div class="tabbar">
        <div id="agent-meta"></div>
        <div class="actions">
          <button id="commands">Commands</button>
          <button id="copy-message">Copy message</button>
          <button id="fork-message">Fork message</button>
          <button id="compact">Compact</button>
          <button id="cancel">Cancel</button>
        </div>
      </div>
      <div class="plan-strip" id="plan"></div>
      <div class="transcript" id="transcript"></div>
      <div id="history-search" hidden></div>
      <form class="composer" id="composer-form">
        <textarea id="composer" rows="3" placeholder="输入消息；Up/Down 历史，Ctrl+R 搜索"></textarea>
        <button type="submit">发送</button>
      </form>
    </section>`;
  app.append(shell);

  shell.querySelectorAll<HTMLButtonElement>("nav button").forEach((button) => {
    button.onclick = () => {
      state.view = button.dataset.view as View;
      render();
    };
  });
  shell.querySelector<HTMLButtonElement>("#compact")!.onclick = compactAgent;
  shell.querySelector<HTMLButtonElement>("#commands")!.onclick = openCommandPalette;
  shell.querySelector<HTMLButtonElement>("#copy-message")!.onclick = legacyOpenMessagePicker;
  shell.querySelector<HTMLButtonElement>("#fork-message")!.onclick = () => void openForkMessagePicker();
  shell.querySelector<HTMLButtonElement>("#resume-agent")!.onclick = openResumePicker;
  shell.querySelector<HTMLButtonElement>("#cancel")!.onclick = cancelAgent;
  const form = shell.querySelector<HTMLFormElement>("#composer-form")!;
  form.onsubmit = (event) => {
    event.preventDefault();
    sendDraft();
  };
  const composer = shell.querySelector<HTMLTextAreaElement>("#composer")!;
  composer.value = state.composerDrafts.get(state.activeAgent) ?? "";
  composer.addEventListener("keydown", (event) => {
    const agent = state.activeAgent;
    const search = agent ? state.historySearches.get(agent) : undefined;
    if (search?.active) {
      if (event.key === "Escape") {
        event.preventDefault();
        closePromptSearch(agent!, composer, true);
        return;
      }
      if (event.key === "Enter" && !event.shiftKey && !event.altKey) {
        event.preventDefault();
        closePromptSearch(agent!, composer, false);
        return;
      }
      if (event.key === "ArrowUp") {
        event.preventDefault();
        search.selected = Math.min(search.selected + 1, Math.max(0, search.matches.length - 1));
        applyPromptMatch(agent!, composer, search);
        syncHistorySearchUi(agent!, composer);
        return;
      }
      if (event.key === "ArrowDown") {
        event.preventDefault();
        search.selected = Math.max(0, search.selected - 1);
        applyPromptMatch(agent!, composer, search);
        syncHistorySearchUi(agent!, composer);
        return;
      }
      if (event.key === "Backspace") {
        event.preventDefault();
        search.query = search.query.slice(0, -1);
        updatePromptSearch(agent!, composer);
        return;
      }
      if (event.key.length === 1 && !event.ctrlKey && !event.metaKey && !event.altKey) {
        event.preventDefault();
        search.query += event.key;
        updatePromptSearch(agent!, composer);
      }
      return;
    }
    if (event.ctrlKey && event.key.toLowerCase() === "r") {
      event.preventDefault();
      if (agent) startPromptSearch(agent, composer);
      return;
    }
    if (event.key === "Enter" && !event.shiftKey && !event.altKey) {
      event.preventDefault();
      sendDraft();
      return;
    }
    if (event.key === "ArrowUp" && !event.shiftKey && !event.ctrlKey && !event.metaKey && !event.altKey) {
      const history = agent ? promptHistory(agent) : [];
      const cursor = composer.selectionStart ?? 0;
      const atBoundary = !composer.value || cursor === 0 || cursor === composer.value.length;
      if (history.length && atBoundary) {
        event.preventDefault();
        const recall = state.promptRecall.get(agent!);
        if (recall !== null && recall !== undefined) {
          if (recall + 1 < history.length) {
            const next = recall + 1;
            composer.value = history[next];
            composer.setSelectionRange(composer.value.length, composer.value.length);
            state.promptRecall.set(agent!, next);
          }
        } else {
          state.promptDrafts.set(agent!, composer.value);
          const next = history.length - 1;
          composer.value = history[next];
          composer.setSelectionRange(composer.value.length, composer.value.length);
          state.promptRecall.set(agent!, next);
        }
      }
      return;
    }
    if (event.key === "ArrowDown" && !event.shiftKey && !event.ctrlKey && !event.metaKey && !event.altKey) {
      const recall = agent ? state.promptRecall.get(agent) : undefined;
      const history = agent ? promptHistory(agent) : [];
      const cursor = composer.selectionStart ?? 0;
      const atBoundary = !composer.value || cursor === 0 || cursor === composer.value.length;
      if (recall !== null && recall !== undefined && history.length && atBoundary) {
        event.preventDefault();
        if (recall === 0) {
          composer.value = state.promptDrafts.get(agent!) ?? "";
          composer.setSelectionRange(composer.value.length, composer.value.length);
          state.promptRecall.set(agent!, null);
          state.promptDrafts.delete(agent!);
        } else {
          const previous = recall - 1;
          composer.value = history[previous];
          composer.setSelectionRange(composer.value.length, composer.value.length);
          state.promptRecall.set(agent!, previous);
        }
      }
      return;
    }
  });
  composer.addEventListener("input", () => {
    const agent = state.activeAgent;
    if (!agent) return;
    if (state.promptRecall.get(agent) !== null && state.promptRecall.get(agent) !== undefined) {
      state.promptRecall.set(agent, null);
      state.promptDrafts.delete(agent);
    }
  });

  shell.querySelector<HTMLButtonElement>("#new-agent")!.onclick = () => {
    state.spawnOpen = !state.spawnOpen;
    render();
  };
  shell.querySelector<HTMLButtonElement>("#new-session")!.onclick = () => {
    state.sessionCreating = !state.sessionCreating;
    state.sessionCreateTitle = "";
    render();
  };
  shell.querySelector<HTMLButtonElement>("#new-session")!.disabled = !state.activeAgent;

  const agents = shell.querySelector<HTMLDivElement>("#agents")!;
  for (const agent of state.agents) {
    const wrapper = document.createElement("div");
    wrapper.className = `agent ${agent.path === state.activeAgent ? "active" : ""}`;
    const button = document.createElement("button");
    button.innerHTML = `<strong>${escapeHtml(agent.path)}</strong><small>${escapeHtml(statusLabel(agent.path))}</small>`;
    button.onclick = () => {
      storeComposerDraft(state.activeAgent);
      state.activeAgent = agent.path;
      void loadHistory(agent.path, true);
      void loadAgentModel(agent.path);
      render();
    };
    const actions = document.createElement("div");
    actions.className = "agent-actions";
    const shutdown = document.createElement("button");
    shutdown.textContent = state.agentBusy === agent.path ? "…" : "Shutdown";
    shutdown.disabled = Boolean(state.agentBusy);
    shutdown.onclick = () => void shutdownRunningAgent(agent.path);
    const remove = document.createElement("button");
    remove.textContent = "Delete";
    remove.disabled = Boolean(state.agentBusy);
    remove.onclick = () => void deleteRunningAgent(agent.path, agent.agent_id);
    actions.append(shutdown, remove);
    wrapper.append(button, actions);
    agents.append(wrapper);
  }

  const spawnForm = shell.querySelector<HTMLDivElement>("#spawn-form")!;
  if (state.spawnOpen) {
    const form = document.createElement("form");
    form.className = "spawn-form";
    form.innerHTML = `
      <input id="spawn-name" placeholder="agent name" value="${escapeHtml(state.spawnName)}" required />
      <input id="spawn-parent" placeholder="/root" value="${escapeHtml(state.spawnParent)}" required />
      <select id="spawn-profile"></select>
      <button type="submit">Spawn</button>`;
    const select = form.querySelector<HTMLSelectElement>("#spawn-profile")!;
    for (const profile of state.profiles.length ? state.profiles : [state.spawnProfile]) {
      const option = document.createElement("option");
      option.value = String(profile);
      option.textContent = String(profile);
      option.selected = String(profile) === state.spawnProfile;
      select.append(option);
    }
    form.querySelector<HTMLInputElement>("#spawn-name")!.oninput = (event) => {
      state.spawnName = (event.target as HTMLInputElement).value;
    };
    form.querySelector<HTMLInputElement>("#spawn-parent")!.oninput = (event) => {
      state.spawnParent = (event.target as HTMLInputElement).value;
    };
    select.onchange = () => {
      state.spawnProfile = select.value;
    };
    form.onsubmit = (event) => {
      event.preventDefault();
      state.spawnProfile = select.value;
      spawnAgent();
    };
    spawnForm.append(form);
  }

  const sessions = shell.querySelector<HTMLDivElement>("#sessions")!;
  const createArea = shell.querySelector<HTMLDivElement>("#session-create")!;
  if (state.sessionCreating && state.activeAgent) {
    const form = document.createElement("form");
    form.className = "inline-session-form";
    const input = document.createElement("input");
    input.value = state.sessionCreateTitle;
    input.placeholder = "New session title";
    input.required = true;
    input.oninput = () => {
      state.sessionCreateTitle = input.value;
    };
    const save = document.createElement("button");
    save.textContent = "Create";
    const cancel = document.createElement("button");
    cancel.type = "button";
    cancel.textContent = "Cancel";
    cancel.onclick = () => {
      state.sessionCreating = false;
      state.sessionCreateTitle = "";
      render();
    };
    form.append(input, save, cancel);
    form.onsubmit = (event) => {
      event.preventDefault();
      void createSession(state.activeAgent, state.sessionCreateTitle)
        .catch((error) => window.alert(String(error)))
        .finally(render);
    };
    createArea.append(form);
  }
  for (const session of state.sessions[state.activeAgent] ?? []) {
    const active = state.activeSessions.get(state.activeAgent) === session.id;
    const row = document.createElement("div");
    row.className = `session-row ${active ? "active" : ""}`;
    const activate = document.createElement("button");
    activate.type = "button";
    activate.innerHTML = `
      <span>${escapeHtml(session.title || session.id)}</span>
      <small>${escapeHtml(new Date(session.last_active).toLocaleString())}${active ? " · active" : ""}</small>`;
    activate.onclick = () => {
      state.activeSessions.set(state.activeAgent, session.id);
      request("POST", `/agents/${state.activeAgent}/sessions/${session.id}/activate`)
        .then(() => {
          state.transcripts.set(state.activeAgent, []);
          state.streaming.delete(state.activeAgent);
          state.historyLoaded.delete(state.activeAgent);
          state.plans.delete(state.activeAgent);
          state.usage.delete(state.activeAgent);
          void loadHistory(state.activeAgent, true);
        })
        .catch((error) => window.alert(String(error)))
        .finally(render);
    };
    const actions = document.createElement("div");
    actions.className = "session-actions";
    const rename = document.createElement("button");
    rename.textContent = "Rename";
    rename.onclick = () => {
      state.sessionEditor = {
        agent: state.activeAgent,
        id: session.id,
        title: session.title || "",
      };
      render();
    };
    const close = document.createElement("button");
    close.textContent = "Close";
    close.onclick = () => {
      void closeSession(state.activeAgent, session.id)
        .catch((error) => window.alert(String(error)))
        .finally(render);
    };
    actions.append(rename, close);
    row.append(activate, actions);
    sessions.append(row);
    if (
      state.sessionEditor &&
      state.sessionEditor.agent === state.activeAgent &&
      state.sessionEditor.id === session.id
    ) {
      const editor = document.createElement("form");
      editor.className = "inline-session-form";
      const input = document.createElement("input");
      input.value = state.sessionEditor.title;
      input.placeholder = "Session title";
      input.required = true;
      input.oninput = () => {
        if (state.sessionEditor) state.sessionEditor.title = input.value;
      };
      const save = document.createElement("button");
      save.textContent = "Save";
      const cancel = document.createElement("button");
      cancel.type = "button";
      cancel.textContent = "Cancel";
      cancel.onclick = () => {
        state.sessionEditor = null;
        render();
      };
      editor.append(input, save, cancel);
      editor.onsubmit = (event) => {
        event.preventDefault();
        void renameSession(state.activeAgent, session.id, state.sessionEditor?.title ?? "")
          .catch((error) => window.alert(String(error)))
          .finally(render);
      };
      sessions.append(editor);
    }
  }

  const meta = shell.querySelector<HTMLDivElement>("#agent-meta")!;
  const activeInfo = state.agents.find((item) => item.path === state.activeAgent);
  const modelLabel = activeInfo?.model || state.activeModel || "no model";
  const contextLabel = activeInfo?.context_length ? ` · ${activeInfo.context_length.toLocaleString()} ctx` : "";
  meta.innerHTML = `<strong>${escapeHtml(state.activeAgent || "No agent")}</strong><span>${escapeHtml(statusLabel(state.activeAgent))}</span><span>${escapeHtml(`${modelLabel}${contextLabel}`)}</span>`;
  const usage = state.usage.get(state.activeAgent);
  if (usage) {
    meta.innerHTML += `<span>${usage.input} in / ${usage.output} out</span>`;
  }
  const connection = shell.querySelector<HTMLDivElement>("#connection")!;
  connection.textContent = state.connected ? "Gateway 已连接" : "已断开";
  connection.dataset.state = state.connected ? "online" : "offline";
  shell.querySelectorAll("nav button").forEach((button) => {
    button.classList.toggle("active", button.getAttribute("data-view") === state.view);
  });

  const plan = state.plans.get(state.activeAgent);
  const planStrip = shell.querySelector<HTMLDivElement>("#plan")!;
  const steps: any[] = plan?.update?.plan ?? [];
  planStrip.textContent = steps.length
    ? steps.map((step) => `${step.status === "completed" ? "✓" : "•"} ${step.step}`).join("    ")
    : "";

  const view = shell.querySelector<HTMLDivElement>("#transcript")!;
  if (state.view === "agent") {
    view.className = "transcript";
    renderTranscript(view);
  } else if (state.view === "config") {
    view.classList.add("config");
    void loadAgentConfig(state.activeAgent);
    void loadEmbeddingConfig();
    renderConfig(view);
  } else if (state.view === "bib") {
    void loadBibliography();
    renderBibliography(view);
  } else if (state.view === "dag") {
    void loadDag(state.activeAgent);
    renderDag(view);
  }
  if (state.view === "kms") {
    void loadKms();
    renderKms(view);
  }
  if (statusLabel(state.activeAgent).match(/requesting|streaming|executing/i)) {
    composer.placeholder = "agent 运行中；输入可加入 runtime 队列";
  }
  syncHistorySearchUi(state.activeAgent, composer);
  renderMessagePickerModal();
  renderResumePickerModal();
  renderCommandPaletteModal();
  renderBibRequestsModal();
  renderBibImportModal();
  if (lastFrontendReadyStage !== "app") {
    const geometry = (selector: string) => {
      const element = shell.querySelector(selector);
      if (!element) return null;
      const rect = element.getBoundingClientRect();
      const style = getComputedStyle(element);
      return {
        x: rect.x,
        y: rect.y,
        width: rect.width,
        height: rect.height,
        display: style.display,
        visibility: style.visibility,
        opacity: style.opacity,
      };
    };
    const hit = document.elementFromPoint(
      window.innerWidth / 2,
      window.innerHeight / 2,
    );
    reportFrontendReady("app", {
      connected: state.connected,
      view: state.view,
      activeAgent: state.activeAgent,
      agents: state.agents.length,
      sessionAgents: Object.keys(state.sessions).length,
      transcriptChildren: view.childElementCount,
      shellChildren: shell.childElementCount,
      bodyTextChars: shell.textContent?.length ?? 0,
      viewport: {
        width: window.innerWidth,
        height: window.innerHeight,
      },
      documentRect: {
        width: document.documentElement.clientWidth,
        height: document.documentElement.clientHeight,
      },
      appRect: geometry("#app"),
      shellRect: geometry(".shell"),
      headerRect: geometry("header"),
      asideRect: geometry("aside"),
      workspaceRect: geometry(".workspace"),
      transcriptRect: geometry("#transcript"),
      composerRect: geometry("#composer"),
      centerHit: hit
        ? {
            tag: hit.tagName,
            className: hit.className,
            text: (hit.textContent ?? "").slice(0, 120),
          }
        : null,
    });
    lastFrontendReadyStage = "app";
  }
}

window.addEventListener("keydown", (event) => {
  if (
    event.ctrlKey
    && event.key.toLowerCase() === "p"
    && !state.messagePicker.open
    && !state.resumePicker.open
  ) {
    event.preventDefault();
    openCommandPalette();
  }
});

void hydrate();
