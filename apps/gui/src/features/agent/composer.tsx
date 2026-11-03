import { useEffect, useState } from "react";
import {
  Archive,
  ArrowUp,
  Brain,
  Check,
  ChevronDown,
  CircleSlash,
  Cpu,
  GitFork,
  Square,
} from "lucide-react";
import type { ModelRow } from "@/types";
import { Button } from "@/components/ui/button";
import {
  Command,
  CommandGroup,
  CommandInput,
  CommandItem,
  CommandList,
} from "@/components/ui/command";
import { Textarea } from "@/components/ui/textarea";
import { cn } from "@/lib/utils";
import { agentStatusTone, statusLabel } from "./agent";

/** Reasoning-effort ladder; mirrors `ReasoningEffort` in
 *  crates/agentik-types/src/reasoning.rs — every variant must appear. */
const EFFORT_OPTIONS = [
  { value: "none", label: "关闭", hint: "none" },
  { value: "minimal", label: "极简", hint: "minimal" },
  { value: "low", label: "低", hint: "low" },
  { value: "medium", label: "中", hint: "medium" },
  { value: "high", label: "高", hint: "high" },
  { value: "xhigh", label: "超高", hint: "xhigh" },
  { value: "max", label: "最大", hint: "max" },
  { value: "ultra", label: "极致", hint: "ultra" },
] as const;

function effortLabel(effort: string | null | undefined) {
  return EFFORT_OPTIONS.find((option) => option.value === effort)?.label ?? "推理";
}

/** Lifecycle states that mean "no turn in flight" — anything else keeps
 *  the composer's send button in cancel mode with the status ring. */
const TERMINAL_LIFECYCLE = new Set([
  "idle",
  "error",
  "cancelled",
  "aborted",
  "completed",
]);

export type AgentComposerProps = {
  activeAgent: string;
  draft: string;
  /** In-flight POST marker (`"send"` covers the window before the first
   *  lifecycle event arrives). */
  busy: string | null;
  /** True while a streaming delta is attached (text arriving between
   *  lifecycle events). */
  streamingActive: boolean;
  /** Lifecycle status of the active agent, e.g. "idle" / "Streaming". */
  status?: string;
  /** Active agent's current `provider:model` spec (drives the pill label
   *  and the checked row in the switcher). */
  currentModelSpec: string | null;
  /** Cached catalog rows (all providers) for the composer model switcher. */
  modelCatalog: ModelRow[];
  /** Fetches the catalog once; the switcher awaits it before opening so
   *  the list never shows a stale empty state. */
  onLoadModelCatalog: () => Promise<void>;
  /** Hot-swaps the agent's model to a `provider:model` spec. */
  onChangeModel: (path: string, spec: string) => Promise<void>;
  /** Effective reasoning effort for the active agent (`undefined` = not
   *  loaded yet, `null` = the model has no thinking support). */
  agentEffort?: string | null;
  /** Re-reads the effective effort; called when the active agent changes
   *  and after a model switch. */
  onRefreshEffort: () => void;
  /** Sets the per-agent reasoning-effort override. */
  onChangeEffort: (path: string, effort: string) => Promise<void>;
  onDraftChange: (value: string) => void;
  onSend: () => void;
  /** 终止当前回复(POST /agents/{id}/cancel)。回合进行中发送按钮变成
   *  终止键;取消后后端发 LifecycleChanged(Cancelled),composer 露出
   *  「已终止」状态直到下一个回合。 */
  onCancel: () => void;
  /** 压缩对话(POST /agents/{id}/compact)——composer 内的 Archive 图标。 */
  onCompact: () => void;
  onOpenDag: () => void;
};

/** 回合操作中心:消息输入 + 发送/终止键 + 模型/推理力度切换 + 压缩/DAG。
 *  回合进行中发送键变终止键,外圈虚线环旋转,颜色随生命周期状态。 */
export function AgentComposer(props: AgentComposerProps) {
  const { activeAgent, draft, busy, streamingActive, status } = props;

  const [modelPickerOpen, setModelPickerOpen] = useState(false);
  const [effortPickerOpen, setEffortPickerOpen] = useState(false);
  // Set when the catalog fetch fails; the open list shows the error
  // instead of an empty-state that would read as "no models exist".
  const [modelPickerError, setModelPickerError] = useState<string | null>(null);

  // Composer turn state: a turn is in flight unless the lifecycle rests on
  // a terminal state — busy covers the POST window before the first
  // lifecycle event, streaming covers text deltas between events.
  const lifecycle = (status ?? "idle").toLowerCase();
  const turnActive =
    busy === "send" || streamingActive || !TERMINAL_LIFECYCLE.has(lifecycle);
  const terminated =
    !turnActive && (lifecycle.includes("cancel") || lifecycle.includes("abort"));
  const tone = agentStatusTone(status);
  const running = statusLabel(status);
  const currentModelShort = props.currentModelSpec
    ? (props.currentModelSpec.split(":").filter(Boolean).pop() ?? props.currentModelSpec)
    : null;

  const toggleModelPicker = async () => {
    if (modelPickerOpen) {
      setModelPickerOpen(false);
      return;
    }
    setEffortPickerOpen(false);
    setModelPickerError(null);
    try {
      await props.onLoadModelCatalog();
    } catch (cause) {
      setModelPickerError(String(cause));
    }
    setModelPickerOpen(true);
  };

  const toggleEffortPicker = () => {
    if (effortPickerOpen) {
      setEffortPickerOpen(false);
      return;
    }
    setModelPickerOpen(false);
    setEffortPickerOpen(true);
  };

  // Re-read the effective effort whenever the active agent changes; the
  // model switch refreshes its own path.
  useEffect(() => {
    if (!activeAgent) return;
    props.onRefreshEffort();
  }, [activeAgent, props.onRefreshEffort]);

  // Click-outside closes whichever composer popover is open. pointerdown
  // (capture) so the close lands before any click the same gesture
  // triggers elsewhere — e.g. the send button submits while the list is
  // already folding away. Both pills stay excluded: their own clicks go
  // through the toggles.
  useEffect(() => {
    if (!modelPickerOpen && !effortPickerOpen) return;
    const onPointerDown = (event: PointerEvent) => {
      if (!(event.target instanceof Element)) return;
      if (event.target.closest("[data-composer-popover]")) return;
      setModelPickerOpen(false);
      setEffortPickerOpen(false);
    };
    document.addEventListener("pointerdown", onPointerDown, true);
    return () => document.removeEventListener("pointerdown", onPointerDown, true);
  }, [modelPickerOpen, effortPickerOpen]);

  return (
    <form
      className="relative shrink-0 border-t bg-muted p-2 sm:p-3"
      onSubmit={(event) => {
        event.preventDefault();
        props.onSend();
      }}
    >
      {/* Model switcher: floats above the composer card so the
          textarea keeps focus-relevant layout; the list reuses the
          same Command shape as the agent picker. */}
      {modelPickerOpen && (
        <div
          data-composer-popover=""
          className="absolute inset-x-2 bottom-full z-30 mb-2 overflow-hidden rounded-xl border bg-popover text-popover-foreground shadow-lg sm:inset-x-3"
        >
          <Command
            loop
            onKeyDown={(event) => {
              if (event.key === "Escape") {
                event.stopPropagation();
                setModelPickerOpen(false);
              }
            }}
          >
            <div className="flex h-9 items-center gap-1.5 border-b px-1.5">
              <Cpu className="size-3.5 shrink-0 text-muted-foreground" />
              <div className="min-w-0 flex-1 [&>div]:p-0">
                <CommandInput autoFocus placeholder="搜索模型，Enter 切换，Esc 关闭" />
              </div>
            </div>
            <CommandList className="max-h-64">
              {modelPickerError ? (
                <div className="p-4 text-xs text-destructive">模型目录加载失败: {modelPickerError}</div>
              ) : (
                <>
                  {!props.modelCatalog.length && (
                    <div className="p-4 text-center text-sm text-muted-foreground">
                      模型目录为空，请先在「设置 → 模型」中添加
                    </div>
                  )}
                  <CommandGroup>
                    {props.modelCatalog.map((model) => {
                      const spec = `${model.provider_name}:${model.model_name}`;
                      const active = spec === props.currentModelSpec;
                      return (
                        <CommandItem
                          key={spec}
                          value={`${model.model_name} ${model.provider_name}`}
                          data-checked={active ? "true" : undefined}
                          onSelect={() => {
                            setModelPickerOpen(false);
                            if (!active) void props.onChangeModel(activeAgent, spec);
                          }}
                        >
                          <span className="min-w-0 flex-1 truncate">{model.model_name}</span>
                          <span className="shrink-0 pl-2 text-xs text-muted-foreground">
                            {model.provider_name}
                          </span>
                        </CommandItem>
                      );
                    })}
                  </CommandGroup>
                </>
              )}
            </CommandList>
          </Command>
        </div>
      )}
      {/* Reasoning-effort switcher: fixed eight-step ladder, no
          search; hidden behind a disabled pill when the model lacks
          thinking support. */}
      {effortPickerOpen && (
        <div
          data-composer-popover=""
          className="absolute bottom-full left-2 z-30 mb-2 w-48 overflow-hidden rounded-xl border bg-popover text-popover-foreground shadow-lg sm:left-3"
        >
          {props.agentEffort === null ? (
            <div className="p-3 text-xs text-muted-foreground">当前模型不支持思考</div>
          ) : (
            <div className="p-1">
              {EFFORT_OPTIONS.map((option) => {
                const active = props.agentEffort === option.value;
                return (
                  <button
                    key={option.value}
                    type="button"
                    onClick={() => {
                      setEffortPickerOpen(false);
                      if (!active) void props.onChangeEffort(activeAgent, option.value);
                    }}
                    data-checked={active ? "true" : undefined}
                    className="flex w-full items-center gap-2 rounded-md px-2 py-1.5 text-sm outline-none hover:bg-accent focus-visible:bg-accent"
                  >
                    <span>{option.label}</span>
                    <span className="text-xs text-muted-foreground">{option.hint}</span>
                    {active && <Check className="ml-auto size-3.5 shrink-0" />}
                  </button>
                );
              })}
            </div>
          )}
        </div>
      )}
      {/* Card-style composer: borderless textarea with a floating
          round send button, meta row below (model / hints / streaming). */}
      <div className="rounded-xl border border-input bg-background transition-colors focus-within:border-ring focus-within:ring-3 focus-within:ring-ring/50">
        <div className="flex items-end gap-2 px-2.5 pt-2">
          <Textarea
            value={draft}
            rows={1}
            onChange={(event) => props.onDraftChange(event.target.value)}
            onKeyDown={(event) => {
              if ((event.ctrlKey || event.metaKey) && event.key === "Enter" && draft.trim() && busy !== "send") {
                event.preventDefault();
                props.onSend();
              }
            }}
            placeholder="输入消息…"
            className="max-h-48 min-h-9 resize-none overflow-y-auto border-0 bg-transparent px-0 py-1 focus-visible:ring-0 md:text-sm dark:bg-transparent"
          />
          {turnActive ? (
            /* 回合进行中:发送键变终止键,外圈虚线环旋转,颜色跟随
             * 生命周期状态(流式=天蓝、工具=琥珀、等待=紫、错误=红)。 */
            <span className="relative mb-0.5 flex shrink-0">
              <span
                aria-hidden
                data-status-ring=""
                className={cn(
                  "pointer-events-none absolute -inset-1 animate-spin rounded-full border-2 border-dashed motion-reduce:animate-none",
                  tone.border,
                )}
              />
              <Button
                type="button"
                size="icon"
                variant="secondary"
                className="rounded-full"
                onClick={props.onCancel}
                aria-label="终止"
                title={`终止当前回复（状态：${running || "running"}）`}
              >
                <Square className="size-3.5 fill-current" />
              </Button>
            </span>
          ) : (
            <Button
              type="submit"
              size="icon"
              className="mb-0.5 shrink-0 rounded-full"
              disabled={!draft.trim()}
              aria-label="发送"
              title="发送（Ctrl+Enter）"
            >
              <ArrowUp />
            </Button>
          )}
        </div>
        <div className="flex items-center gap-2 px-2.5 pb-2 pt-1 text-[11px] text-muted-foreground">
          <button
            type="button"
            data-composer-popover=""
            onClick={() => void toggleModelPicker()}
            aria-expanded={modelPickerOpen}
            aria-label="切换模型"
            title={props.currentModelSpec ? `当前模型 ${props.currentModelSpec}，点击切换` : "选择模型"}
            className={cn(
              "flex max-w-[45%] shrink-0 items-center gap-1 rounded-md border border-transparent px-1.5 py-0.5 transition hover:border-border hover:bg-background",
              modelPickerOpen && "border-border bg-background",
            )}
          >
            <Cpu className="size-3 shrink-0" />
            <span className="truncate">{currentModelShort ?? "选择模型"}</span>
            <ChevronDown className={cn("size-3 shrink-0 transition-transform", modelPickerOpen && "rotate-180")} />
          </button>
          <button
            type="button"
            data-composer-popover=""
            onClick={toggleEffortPicker}
            aria-expanded={effortPickerOpen}
            aria-label="推理力度"
            disabled={props.agentEffort === null}
            title={props.agentEffort === null ? "当前模型不支持思考" : `推理力度：${effortLabel(props.agentEffort)}，点击切换`}
            className={cn(
              "flex shrink-0 items-center gap-1 rounded-md border border-transparent px-1.5 py-0.5 transition hover:border-border hover:bg-background disabled:pointer-events-none disabled:opacity-50",
              effortPickerOpen && "border-border bg-background",
            )}
          >
            <Brain className="size-3 shrink-0" />
            <span className="truncate">{effortLabel(props.agentEffort)}</span>
            <ChevronDown className={cn("size-3 shrink-0 transition-transform", effortPickerOpen && "rotate-180")} />
          </button>
          <span className="truncate">Ctrl+Enter 发送 · Ctrl+R 搜索 prompt 历史</span>
          {/* 回合操作收进 composer:压缩 / DAG(fork);右侧是进行中
           * 或已终止的状态提示。 */}
          <div className="ml-auto flex shrink-0 items-center gap-0.5">
            <Button
              size="icon-xs"
              variant="ghost"
              aria-label="压缩对话"
              title="压缩对话（Compact）"
              disabled={!activeAgent}
              onClick={props.onCompact}
            >
              <Archive className="size-3.5" />
            </Button>
            <Button
              size="icon-xs"
              variant="ghost"
              aria-label="查看 agent DAG"
              title="查看 agent DAG"
              disabled={!activeAgent}
              onClick={props.onOpenDag}
            >
              <GitFork className="size-3.5" />
            </Button>
            {turnActive ? (
              <span
                className={cn("flex items-center gap-1 pl-1", tone.text)}
                title={`回合进行中：${running || "running"}`}
              >
                <span className="size-1.5 animate-pulse rounded-full bg-current motion-reduce:animate-none" />
                {running || "running"}…
              </span>
            ) : terminated ? (
              <span className="flex items-center gap-1 pl-1 text-muted-foreground" title="上一回合已被终止">
                <CircleSlash className="size-3" />
                已终止
              </span>
            ) : null}
          </div>
        </div>
      </div>
    </form>
  );
}
