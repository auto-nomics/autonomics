import { useCallback, useEffect, useMemo, useState, type ReactNode } from "react";
import { gatewayRequest } from "@/gateway";
import type {
  AgentRuntimeConfigView,
  AgentRuntimeOverrides,
  CustomModelDraft,
  EmbeddingConfig,
  ModelRow,
  ProviderRow,
  StateSnapshot,
} from "@/types";
import { Button } from "@/components/ui/button";
import { Card } from "@/components/ui/card";
import { Input } from "@/components/ui/input";
import { ScrollArea } from "@/components/ui/scroll-area";
import { Separator } from "@/components/ui/separator";
import { Badge } from "@/components/ui/badge";
import { Tabs, TabsContent, TabsList, TabsTrigger } from "@/components/ui/tabs";
import { cn } from "@/lib/utils";

type Catalog = StateSnapshot["model_catalog"];
type DisplaySettings = StateSnapshot["display_settings"];
type Status = { kind: "ok" | "error"; text: string } | null;

const inputClass = "h-8 w-full rounded-md border border-input bg-background px-2 text-sm";
const selectClass = `${inputClass} appearance-none`;

function Field({ label, children }: { label: string; children: ReactNode }) {
  return (
    <label className="grid gap-1.5">
      <span className="text-xs text-muted-foreground">{label}</span>
      {children}
    </label>
  );
}

function Toggle({ checked, onChange, label }: { checked: boolean; onChange: (checked: boolean) => void; label: string }) {
  return (
    <label className="flex items-center gap-2 text-sm">
      <input
        type="checkbox"
        checked={checked}
        onChange={(event) => onChange(event.target.checked)}
        className="size-4 accent-[var(--primary)]"
      />
      {label}
    </label>
  );
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

function providerModels(catalog: Catalog, provider: string): ModelRow[] {
  return catalog.models.filter((model) => model.provider_name === provider);
}

function supportsRemoteCatalog(providerType: string) {
  return ["openai", "openrouter", "ollama"].includes(providerType);
}

export function ConfigView({ activeAgent }: { activeAgent: string }) {
  const [catalog, setCatalog] = useState<Catalog>({ providers: [], models: [], active_model: null });
  const [display, setDisplay] = useState<DisplaySettings>({
    collapse_thinking: false,
    collapse_tool_calls: false,
    collapse_tool_results: false,
  });
  const [runtime, setRuntime] = useState<AgentRuntimeOverrides>({});
  const [effective, setEffective] = useState<AgentRuntimeConfigView["effective"] | null>(null);
  const [embedding, setEmbedding] = useState<EmbeddingConfig>({
    enabled: true,
    base_url: "http://127.0.0.1:11434",
    model: "qwen3-embedding:0.6b",
    batch_size: 32,
    timeout_secs: 30,
  });
  const [expanded, setExpanded] = useState<Set<string>>(new Set());
  const [credentialProvider, setCredentialProvider] = useState("");
  const [apiKey, setApiKey] = useState("");
  const [baseUrl, setBaseUrl] = useState("");
  const [modelForm, setModelForm] = useState<CustomModelDraft | null>(null);
  const [confirmDelete, setConfirmDelete] = useState<string | null>(null);
  const [catalogBusy, setCatalogBusy] = useState<string | null>(null);
  const [status, setStatus] = useState<Status>(null);
  const [runtimeSaving, setRuntimeSaving] = useState(false);
  const [embeddingSaving, setEmbeddingSaving] = useState(false);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);

  const refreshCatalog = useCallback(async () => {
    const next = await gatewayRequest("GET", "/model-config") as Catalog;
    setCatalog(next);
    return next;
  }, []);

  const loadAll = useCallback(async (agent: string) => {
    setLoading(true);
    setError(null);
    try {
      const [catalogResponse, stateResponse, embeddingResponse] = await Promise.all([
        gatewayRequest("GET", "/model-config"),
        gatewayRequest("GET", "/state"),
        gatewayRequest("GET", "/embedding-config"),
      ]);
      setCatalog(catalogResponse);
      setDisplay(stateResponse.display_settings);
      setEmbedding(embeddingResponse);
      if (agent) {
        const runtimeResponse = await gatewayRequest("GET", `/agents/${agent}/config`) as AgentRuntimeConfigView;
        setRuntime(runtimeResponse.runtime);
        setEffective(runtimeResponse.effective);
      } else {
        setRuntime({});
        setEffective(null);
      }
    } catch (cause) {
      setError(String(cause));
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => {
    void loadAll(activeAgent);
  }, [activeAgent, loadAll]);

  const modelsFor = useMemo(() => (provider: string) => providerModels(catalog, provider), [catalog]);

  const saveProvider = async (providerName: string) => {
    const provider = catalog.providers.find((item) => item.name === providerName);
    if (!provider) return;
    setStatus({ kind: "ok", text: "Saving credentials…" });
    try {
      await gatewayRequest("PUT", "/model-config/provider", {
        name: providerName,
        provider_type: provider.provider_type,
        base_url: baseUrl,
        api_key: apiKey,
      });
      await refreshCatalog();
      setCredentialProvider("");
      setStatus({ kind: "ok", text: `Saved credentials for ${providerName}` });
    } catch (cause) {
      setStatus({ kind: "error", text: String(cause) });
    }
  };

  const saveModel = async () => {
    if (!modelForm) return;
    const modelName = modelForm.model_name.trim();
    const contextLength = Number(modelForm.context_length);
    const maxOutput = Number(modelForm.max_output_tokens);
    const inputPrice = Number(modelForm.input_token_price);
    const outputPrice = Number(modelForm.output_token_price);
    if (!modelName) return setStatus({ kind: "error", text: "Model name is required" });
    if (!Number.isSafeInteger(contextLength) || contextLength <= 0) return setStatus({ kind: "error", text: "Context length must be positive" });
    if (!Number.isSafeInteger(maxOutput) || maxOutput <= 0) return setStatus({ kind: "error", text: "Max output must be positive" });
    if (![inputPrice, outputPrice].every((value) => Number.isFinite(value) && value >= 0)) {
      return setStatus({ kind: "error", text: "Prices must be non-negative numbers" });
    }
    setStatus({ kind: "ok", text: "Saving model…" });
    try {
      await gatewayRequest("PUT", "/model-config/model", {
        provider_name: modelForm.provider_name,
        original_model_name: modelForm.original_model_name,
        model_name: modelName,
        context_length: contextLength,
        max_output_tokens: maxOutput,
        vision_ability: modelForm.vision_ability,
        supports_function_calling: modelForm.supports_function_calling,
        supports_streaming: modelForm.supports_streaming,
        supports_thinking: modelForm.supports_thinking,
        thinking_enabled: modelForm.thinking_enabled,
        input_token_price: inputPrice,
        output_token_price: outputPrice,
      });
      await refreshCatalog();
      setModelForm(null);
      setStatus({ kind: "ok", text: `${modelForm.original_model_name ? "Updated" : "Added"} ${modelForm.provider_name}:${modelName}` });
    } catch (cause) {
      setStatus({ kind: "error", text: String(cause) });
    }
  };

  const deleteModel = async (providerName: string, modelName: string) => {
    const spec = `${providerName}:${modelName}`;
    if (confirmDelete !== spec) return setConfirmDelete(spec);
    try {
      await gatewayRequest(
        "DELETE",
        `/model-config/model?provider_name=${encodeURIComponent(providerName)}&model_name=${encodeURIComponent(modelName)}`,
      );
      await refreshCatalog();
      setConfirmDelete(null);
      setStatus({ kind: "ok", text: `Deleted ${spec}` });
    } catch (cause) {
      setStatus({ kind: "error", text: String(cause) });
    }
  };

  const setActiveModel = async (spec: string) => {
    try {
      await gatewayRequest("PUT", "/model-config/active-model", { spec });
      setCatalog((current) => ({ ...current, active_model: spec }));
    } catch (cause) {
      window.alert(String(cause));
    }
  };

  const fetchCatalog = async (provider: ProviderRow) => {
    setCatalogBusy(provider.name);
    setStatus({ kind: "ok", text: `Fetching ${provider.name} catalogue…` });
    try {
      await gatewayRequest("POST", `/model-config/providers/${encodeURIComponent(provider.name)}/catalog`, {
        base_url: provider.base_url,
      });
      setStatus({ kind: "ok", text: `Fetching ${provider.name} catalogue in daemon…` });
    } catch (cause) {
      setStatus({ kind: "error", text: String(cause) });
    } finally {
      setCatalogBusy(null);
    }
  };

  const saveRuntime = async () => {
    if (!activeAgent) return;
    setRuntimeSaving(true);
    try {
      const response = await gatewayRequest("PUT", `/agents/${activeAgent}/config`, { runtime }) as AgentRuntimeConfigView;
      setRuntime(response.runtime);
      setEffective(response.effective);
      setStatus({ kind: "ok", text: "Agent runtime saved" });
    } catch (cause) {
      setStatus({ kind: "error", text: String(cause) });
    } finally {
      setRuntimeSaving(false);
    }
  };

  const saveDisplay = async (key: keyof DisplaySettings, value: boolean) => {
    const previous = display[key];
    setDisplay((current) => ({ ...current, [key]: value }));
    try {
      await gatewayRequest("PUT", "/settings", { key, value: value ? "1" : "0" });
    } catch (cause) {
      setDisplay((current) => ({ ...current, [key]: previous }));
      window.alert(String(cause));
    }
  };

  const saveEmbedding = async () => {
    setEmbeddingSaving(true);
    try {
      const response = await gatewayRequest("PUT", "/embedding-config", { config: embedding });
      setEmbedding(response.config ?? embedding);
      setStatus({ kind: "ok", text: "Embedding model saved" });
    } catch (cause) {
      setStatus({ kind: "error", text: String(cause) });
    } finally {
      setEmbeddingSaving(false);
    }
  };

  const chatgptLogin = async () => {
    try {
      const response = await gatewayRequest("POST", "/model-config/chatgpt/login");
      setStatus({ kind: "ok", text: `Authorize in the browser: ${response.url}` });
      window.open(response.url, "_blank", "noopener");
    } catch (cause) {
      setStatus({ kind: "error", text: String(cause) });
    }
  };

  const refreshChatgpt = async () => {
    try {
      const response = await gatewayRequest("POST", "/model-config/chatgpt/refresh");
      await refreshCatalog();
      setStatus({ kind: response.refreshed ? "ok" : "error", text: response.refreshed ? "ChatGPT token refreshed" : "Token refresh was not needed" });
    } catch (cause) {
      setStatus({ kind: "error", text: String(cause) });
    }
  };

  const runtimeOption = (key: keyof Pick<AgentRuntimeOverrides, "use_memory" | "generate_memory">, option: "inherit" | "on" | "off") => {
    const value = runtime[key];
    const active = option === "inherit" ? value == null : option === "on" ? value === true : value === false;
    return (
      <Button
        key={option}
        type="button"
        size="xs"
        variant={active ? "default" : "ghost"}
        onClick={() => setRuntime((current) => ({ ...current, [key]: option === "inherit" ? null : option === "on" }))}
      >
        {option}
      </Button>
    );
  };

  return (
    <ScrollArea className="min-h-0 flex-1">
      <div className="mx-auto flex w-full max-w-6xl flex-col gap-4 pb-6">
        {loading && <Card className="p-4 text-sm text-muted-foreground">Loading configuration…</Card>}
        {error && <Card className="border-destructive/40 p-4 text-sm text-destructive">{error}</Card>}
        {status && (
          <div className={cn(
            "rounded-lg border p-3 text-sm",
            status.kind === "ok" ? "border-primary/40 text-foreground" : "border-destructive/40 text-destructive",
          )}>
            {status.text}
          </div>
        )}

        <Tabs defaultValue="models" className="gap-4">
          <TabsList>
            <TabsTrigger value="models">Models</TabsTrigger>
            <TabsTrigger value="agent">Agent Runtime</TabsTrigger>
            <TabsTrigger value="settings">Settings</TabsTrigger>
          </TabsList>

          <TabsContent value="models" className="grid gap-4">
            <Card className="p-4">
              <div className="flex items-center justify-between">
                <div>
                  <h3 className="text-sm font-medium">Active model</h3>
                  <p className="text-xs text-muted-foreground">{catalog.active_model ?? "No model selected"}</p>
                </div>
                <Button size="sm" variant="secondary" onClick={() => void refreshCatalog().then(() => setStatus({ kind: "ok", text: "Catalog refreshed" }))}>
                  Refresh
                </Button>
              </div>
            </Card>

            {catalog.providers.map((provider) => {
              const open = expanded.has(provider.name);
              const models = modelsFor(provider.name);
              const token = catalog.openai_token;
              return (
                <Card key={provider.id} className="p-0">
                  <button
                    type="button"
                    className="flex w-full items-center gap-2 p-3 text-left hover:bg-accent"
                    onClick={() => setExpanded((current) => {
                      const next = new Set(current);
                      if (next.has(provider.name)) next.delete(provider.name);
                      else next.add(provider.name);
                      return next;
                    })}
                  >
                    <strong className="text-sm">{provider.name}</strong>
                    <Badge variant="secondary" className="ml-auto">{provider.api_key ? "configured" : "not configured"}</Badge>
                    <span>{open ? "▾" : "▸"}</span>
                  </button>
                  {open && (
                    <div className="space-y-3 border-t p-3">
                      {models.map((model) => {
                        const spec = `${provider.name}:${model.model_name}`;
                        return (
                          <div key={spec} className="flex items-center gap-2 rounded-md border bg-background p-2">
                            <span className={cn("text-xs", catalog.active_model === spec ? "text-primary" : "text-transparent")}>●</span>
                            <span className="min-w-0 flex-1 truncate text-sm">{model.model_name}</span>
                            <span className="text-xs text-muted-foreground">{model.context_length.toLocaleString()} ctx</span>
                            <Button size="xs" variant={catalog.active_model === spec ? "secondary" : "outline"} disabled={catalog.active_model === spec} onClick={() => void setActiveModel(spec)}>
                              {catalog.active_model === spec ? "active" : "activate"}
                            </Button>
                            {model.custom && (
                              <>
                                <Button size="xs" variant="ghost" onClick={() => {
                                  setModelForm({
                                    provider_name: provider.name,
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
                                  });
                                }}>Edit</Button>
                                <Button size="xs" variant={confirmDelete === spec ? "destructive" : "ghost"} onClick={() => void deleteModel(provider.name, model.model_name)}>
                                  {confirmDelete === spec ? "Confirm" : "Delete"}
                                </Button>
                              </>
                            )}
                          </div>
                        );
                      })}
                      {!models.length && <p className="text-sm text-muted-foreground">No models</p>}

                      <div className="flex flex-wrap gap-2">
                        {provider.provider_type !== "openai" && (
                          <Button size="sm" variant="secondary" onClick={() => {
                            setCredentialProvider(credentialProvider === provider.name ? "" : provider.name);
                            setApiKey(provider.api_key);
                            setBaseUrl(provider.base_url);
                          }}>
                            {provider.api_key ? "Edit API key" : "Add API key"}
                          </Button>
                        )}
                        <Button size="sm" variant="outline" onClick={() => setModelForm(modelForm?.provider_name === provider.name && !modelForm.original_model_name ? null : newModelDraft(provider.name))}>
                          {modelForm?.provider_name === provider.name && !modelForm.original_model_name ? "Cancel add" : "Add model"}
                        </Button>
                        {supportsRemoteCatalog(provider.provider_type) && (
                          <Button size="sm" variant="outline" disabled={catalogBusy === provider.name} onClick={() => void fetchCatalog(provider)}>
                            {catalogBusy === provider.name ? "Fetching…" : "Fetch catalog"}
                          </Button>
                        )}
                      </div>

                      {provider.provider_type === "openai" && (
                        <div className="flex flex-wrap items-center gap-2 text-sm text-muted-foreground">
                          <span>
                            ChatGPT: {!token?.present ? "not signed in" : token.expired ? "token expired" : "token active"}
                          </span>
                          <Button size="xs" variant="secondary" onClick={() => void chatgptLogin()}>Login</Button>
                          <Button size="xs" variant="secondary" onClick={() => void refreshChatgpt()}>Refresh token</Button>
                        </div>
                      )}

                      {credentialProvider === provider.name && (
                        <form className="grid gap-3 rounded-md border p-3" onSubmit={(event) => { event.preventDefault(); void saveProvider(provider.name); }}>
                          <Field label="API Key"><Input type="password" value={apiKey} onChange={(event) => setApiKey(event.target.value)} /></Field>
                          <Field label="Base URL"><Input value={baseUrl} onChange={(event) => setBaseUrl(event.target.value)} /></Field>
                          <div className="flex justify-end gap-2">
                            <Button type="button" size="sm" variant="ghost" onClick={() => setCredentialProvider("")}>Cancel</Button>
                            <Button type="submit" size="sm">Save</Button>
                          </div>
                        </form>
                      )}

                      {modelForm?.provider_name === provider.name && (
                        <form className="grid gap-3 rounded-md border p-3" onSubmit={(event) => { event.preventDefault(); void saveModel(); }}>
                          <h4 className="text-sm font-medium">{modelForm.original_model_name ? `Edit ${modelForm.original_model_name}` : "Add custom model"}</h4>
                          <div className="grid gap-3 md:grid-cols-2">
                            <Field label="Model name"><Input value={modelForm.model_name} onChange={(event) => setModelForm({ ...modelForm, model_name: event.target.value })} /></Field>
                            <Field label="Context length"><Input type="number" value={modelForm.context_length} onChange={(event) => setModelForm({ ...modelForm, context_length: event.target.value })} /></Field>
                            <Field label="Max output"><Input type="number" value={modelForm.max_output_tokens} onChange={(event) => setModelForm({ ...modelForm, max_output_tokens: event.target.value })} /></Field>
                            <Field label="Input price"><Input type="number" value={modelForm.input_token_price} onChange={(event) => setModelForm({ ...modelForm, input_token_price: event.target.value })} /></Field>
                            <Field label="Output price"><Input type="number" value={modelForm.output_token_price} onChange={(event) => setModelForm({ ...modelForm, output_token_price: event.target.value })} /></Field>
                          </div>
                          <div className="grid gap-2 sm:grid-cols-2">
                            <Toggle label="Vision" checked={modelForm.vision_ability} onChange={(checked) => setModelForm({ ...modelForm, vision_ability: checked })} />
                            <Toggle label="Function calling" checked={modelForm.supports_function_calling} onChange={(checked) => setModelForm({ ...modelForm, supports_function_calling: checked })} />
                            <Toggle label="Streaming" checked={modelForm.supports_streaming} onChange={(checked) => setModelForm({ ...modelForm, supports_streaming: checked })} />
                            <Toggle label="Thinking support" checked={modelForm.supports_thinking} onChange={(checked) => setModelForm({ ...modelForm, supports_thinking: checked })} />
                            <Toggle label="Thinking enabled" checked={modelForm.thinking_enabled} onChange={(checked) => setModelForm({ ...modelForm, thinking_enabled: checked })} />
                          </div>
                          <div className="flex justify-end gap-2">
                            <Button type="button" variant="ghost" size="sm" onClick={() => setModelForm(null)}>Cancel</Button>
                            <Button type="submit" size="sm">Save model</Button>
                          </div>
                        </form>
                      )}
                    </div>
                  )}
                </Card>
              );
            })}
          </TabsContent>

          <TabsContent value="agent" className="grid gap-4">
            <Card className="p-4">
              <div className="flex items-center justify-between">
                <h3 className="text-sm font-medium">Agent Runtime</h3>
                <Badge variant="secondary">{activeAgent || "no agent"}</Badge>
              </div>
              {!activeAgent ? (
                <p className="mt-3 text-sm text-muted-foreground">Select an agent to edit runtime overrides.</p>
              ) : (
                <div className="mt-4 space-y-4">
                  {(["use_memory", "generate_memory"] as const).map((key) => (
                    <div key={key} className="grid items-center gap-2 sm:grid-cols-[220px_1fr_auto]">
                      <span className="text-sm capitalize">{key.replace("_", " ")}</span>
                      <div className="inline-flex w-fit rounded-md bg-secondary p-0.5">{["inherit", "on", "off"].map((option) => runtimeOption(key, option as any))}</div>
                      <small className="text-xs text-muted-foreground">
                        {key === "use_memory" ? (effective?.use_memory ? "effective on" : "effective off") : (effective?.generate_memory ? "effective on" : "effective off")}
                      </small>
                    </div>
                  ))}
                  <div className="grid items-center gap-2 sm:grid-cols-[220px_1fr_auto]">
                    <span className="text-sm">Reasoning effort</span>
                    <select
                      className={selectClass}
                      value={runtime.reasoning_effort ?? ""}
                      onChange={(event) => setRuntime({ ...runtime, reasoning_effort: (event.target.value || null) as AgentRuntimeOverrides["reasoning_effort"] })}
                    >
                      {["", "none", "minimal", "low", "medium", "high", "xhigh", "max", "ultra"].map((value) => (
                        <option key={value || "inherit"} value={value}>{value || "inherit"}</option>
                      ))}
                    </select>
                    <small className="text-xs text-muted-foreground">{effective?.reasoning_effort ? `effective ${effective.reasoning_effort}` : "effective model default"}</small>
                  </div>
                  <Separator />
                  <div className="flex justify-between">
                    <Button variant="ghost" size="sm" onClick={() => setRuntime({})}>All inherit</Button>
                    <Button size="sm" disabled={runtimeSaving} onClick={() => void saveRuntime()}>{runtimeSaving ? "Saving…" : "Save"}</Button>
                  </div>
                </div>
              )}
            </Card>
          </TabsContent>

          <TabsContent value="settings" className="grid gap-4">
            <Card className="p-4">
              <h3 className="text-sm font-medium">Display</h3>
              <div className="mt-3 grid gap-2">
                <Toggle label="Collapse thinking" checked={display.collapse_thinking} onChange={(checked) => void saveDisplay("collapse_thinking", checked)} />
                <Toggle label="Collapse tool calls" checked={display.collapse_tool_calls} onChange={(checked) => void saveDisplay("collapse_tool_calls", checked)} />
                <Toggle label="Collapse tool results" checked={display.collapse_tool_results} onChange={(checked) => void saveDisplay("collapse_tool_results", checked)} />
              </div>
            </Card>
            <Card className="p-4">
              <div className="flex items-center justify-between">
                <h3 className="text-sm font-medium">Embedding Model</h3>
                <Badge variant="secondary">{embedding.enabled ? "enabled" : "disabled"}</Badge>
              </div>
              <form className="mt-4 grid gap-3 md:grid-cols-2" onSubmit={(event) => { event.preventDefault(); void saveEmbedding(); }}>
                <div className="md:col-span-2"><Toggle label="Enabled" checked={embedding.enabled} onChange={(checked) => setEmbedding({ ...embedding, enabled: checked })} /></div>
                <Field label="Base URL"><Input value={embedding.base_url} onChange={(event) => setEmbedding({ ...embedding, base_url: event.target.value })} /></Field>
                <Field label="Model"><Input value={embedding.model} onChange={(event) => setEmbedding({ ...embedding, model: event.target.value })} /></Field>
                <Field label="Batch size"><Input type="number" min={1} value={embedding.batch_size} onChange={(event) => setEmbedding({ ...embedding, batch_size: Number(event.target.value) })} /></Field>
                <Field label="Timeout seconds"><Input type="number" min={1} value={embedding.timeout_secs} onChange={(event) => setEmbedding({ ...embedding, timeout_secs: Number(event.target.value) })} /></Field>
                <div className="flex justify-end gap-2 md:col-span-2">
                  <Button type="button" variant="ghost" size="sm" onClick={() => void loadAll(activeAgent)}>Reload</Button>
                  <Button type="submit" size="sm" disabled={embeddingSaving}>{embeddingSaving ? "Saving…" : "Save"}</Button>
                </div>
              </form>
            </Card>
          </TabsContent>
        </Tabs>
      </div>
    </ScrollArea>
  );
}
