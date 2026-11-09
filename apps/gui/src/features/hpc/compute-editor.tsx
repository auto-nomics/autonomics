/**
 * 计算目标编辑器:管理 compute registry state(目标/策略/数据源)、检查目标、
 * 查看部署就绪与运行绑定。操作员级操作,经网关 HTTP 完成;目录修订采用
 * 乐观并发(expected_revision),409 冲突时提示并自动重载。
 */
import { useCallback, useEffect, useMemo, useState } from "react";
import { GatewayHttpError } from "@/gateway";
import {
  addComputeDataSource,
  checkComputeTarget,
  fetchComputeReadiness,
  fetchComputeStatus,
  saveComputeTarget,
  selectComputeTarget,
} from "./api";
import type {
  ComputeStatus,
  ComputeTarget,
  ReadinessReport,
  SlurmSettings,
} from "./types";
import { Button } from "@/components/ui/button";
import { Badge } from "@/components/ui/badge";
import { ScrollArea } from "@/components/ui/scroll-area";
import { Separator } from "@/components/ui/separator";
import { cn } from "@/lib/utils";

const inputClass =
  "h-8 w-full rounded-md border border-input bg-background px-2 text-sm";

type Status = { kind: "ok" | "error"; text: string } | null;

type TargetDraft = {
  id: string;
  name: string;
  enabled: boolean;
  /** null = local 后端 */
  remote: { kind: "slurm" | "ssh"; settings: SlurmSettings } | null;
};

function backendLabel(target: ComputeTarget): string {
  if (target.backend.type === "local") return "本地";
  const s = target.backend.settings;
  const kind = target.backend.type === "ssh" ? "SSH" : "Slurm";
  return `${kind} · ${s.ssh_host ?? "?"}${s.typed_worker_config ? " · typed-worker" : ""}`;
}

function emptySlurmDraft(): SlurmSettings {
  return {
    enabled: true,
    ssh_host: "",
    ssh_command: "ssh",
    scratch_remote_dir: "~/autonomics-scratch",
    partition: "64c512g",
    remote_python: "python3",
    tools_enabled: true,
  };
}

function draftFromTarget(target: ComputeTarget): TargetDraft {
  return {
    id: target.id,
    name: target.name,
    enabled: target.enabled,
    remote:
      target.backend.type === "local"
        ? null
        : { kind: target.backend.type, settings: { ...target.backend.settings } },
  };
}

function Field({
  label,
  children,
  hint,
}: {
  label: string;
  children: React.ReactNode;
  hint?: string;
}) {
  return (
    <label className="grid gap-1.5">
      <span className="text-xs text-muted-foreground">{label}</span>
      {children}
      {hint ? (
        <span className="text-[10px] leading-tight text-muted-foreground/70">
          {hint}
        </span>
      ) : null}
    </label>
  );
}

export function ComputeEditorView() {
  const [status, setStatus] = useState<ComputeStatus | null>(null);
  const [loadError, setLoadError] = useState<string | null>(null);
  const [busy, setBusy] = useState<string | null>(null);
  const [notice, setNotice] = useState<Status>(null);
  const [checkReport, setCheckReport] = useState<
    Record<string, ReadinessReport>
  >({});
  const [readiness, setReadiness] = useState<ReadinessReport | null>(null);
  const [draft, setDraft] = useState<TargetDraft | null>(null);
  const [sourceDraft, setSourceDraft] = useState({
    id: "",
    name: "",
    alias: "",
    root: "",
    python: "python3",
  });
  const [showSourceForm, setShowSourceForm] = useState(false);

  const reload = useCallback(async () => {
    try {
      const next = await fetchComputeStatus();
      setStatus(next);
      setLoadError(null);
    } catch (error) {
      setLoadError(errorText(error));
    }
  }, []);

  useEffect(() => {
    void reload();
    const timer = window.setInterval(() => void reload(), 15_000);
    return () => window.clearInterval(timer);
  }, [reload]);

  // 网关通知(compute_changed)到达时立即刷新。
  useEffect(() => {
    const onComputeChanged = () => void reload();
    window.addEventListener("autonomics:compute-changed", onComputeChanged);
    return () =>
      window.removeEventListener("autonomics:compute-changed", onComputeChanged);
  }, [reload]);

  const state = status?.state ?? null;
  const revision = state?.revision ?? 0;
  const selectedId = state?.policy.target_id ?? "";

  const run = useCallback(
    async (key: string, action: () => Promise<void>) => {
      setBusy(key);
      setNotice(null);
      try {
        await action();
      } catch (error) {
        if (error instanceof GatewayHttpError && error.status === 409) {
          setNotice({
            kind: "error",
            text: "计算目录已被其他操作修改,已重新加载;请重试本操作。",
          });
          await reload();
        } else {
          setNotice({ kind: "error", text: errorText(error) });
        }
      } finally {
        setBusy(null);
      }
    },
    [reload],
  );

  const targets = useMemo(() => state?.targets ?? [], [state]);
  const sources = useMemo(() => state?.data_sources ?? [], [state]);
  const runs = useMemo(() => status?.runs ?? [], [status]);

  if (loadError && !status) {
    return (
      <div className="p-4 text-sm text-muted-foreground">
        无法加载计算目录:{loadError}
        <Button className="ml-2" size="sm" onClick={() => void reload()}>
          重试
        </Button>
      </div>
    );
  }
  if (!status || !state) {
    return <div className="p-4 text-sm text-muted-foreground">加载中…</div>;
  }

  return (
    <ScrollArea className="min-h-0 flex-1">
      <div className="mx-auto grid max-w-4xl gap-4 p-4 pb-12">
        <header className="flex items-center justify-between gap-2">
          <div>
            <h2 className="text-base font-semibold">计算目标</h2>
            <p className="text-xs text-muted-foreground">
              目录修订 r{revision} · 当前策略:{selectedId} r
              {state.policy.target_revision}(policy r{state.policy.revision})
            </p>
          </div>
          <div className="flex items-center gap-2">
            <Badge
              variant={
                status.deployment?.source_compatible === true
                  ? "default"
                  : "destructive"
              }
            >
              {status.deployment?.source_compatible === true
                ? "worker 源兼容"
                : "worker 源不兼容"}
            </Badge>
            <Button
              size="sm"
              variant="outline"
              disabled={busy !== null}
              onClick={() =>
                void run("readiness", async () => {
                  setReadiness(await fetchComputeReadiness());
                })
              }
            >
              就绪检查
            </Button>
          </div>
        </header>

        {notice ? (
          <div
            className={cn(
              "rounded-md border px-3 py-2 text-xs",
              notice.kind === "ok"
                ? "border-border bg-muted/40"
                : "border-destructive/40 bg-destructive/10 text-destructive",
            )}
          >
            {notice.text}
          </div>
        ) : null}

        {readiness ? (
          <section className="rounded-lg border p-3 text-xs">
            <div className="mb-1 flex items-center gap-2">
              <span className="font-medium">就绪报告</span>
              <Badge
                variant={
                  readiness.state === "CONFIGURATION_READY"
                    ? "default"
                    : "destructive"
                }
              >
                {readiness.state}
              </Badge>
            </div>
            {readiness.blockers?.length ? (
              <ul className="list-disc pl-4 text-muted-foreground">
                {readiness.blockers.map((blocker, index) => (
                  <li key={index}>
                    {String(
                      blocker.detail ?? blocker.message ?? blocker.code ?? "?",
                    )}
                  </li>
                ))}
              </ul>
            ) : (
              <p className="text-muted-foreground">无阻塞项。</p>
            )}
          </section>
        ) : null}

        <section className="rounded-lg border">
          <div className="flex items-center justify-between border-b px-3 py-2">
            <span className="text-sm font-medium">目标</span>
            <Button
              size="sm"
              variant="outline"
              onClick={() =>
                setDraft({
                  id: "",
                  name: "",
                  enabled: true,
                  remote: { kind: "slurm", settings: emptySlurmDraft() },
                })
              }
            >
              新建 Slurm 目标
            </Button>
            <Button
              size="sm"
              variant="outline"
              onClick={() =>
                setDraft({
                  id: "",
                  name: "",
                  enabled: true,
                  remote: { kind: "ssh", settings: emptySlurmDraft() },
                })
              }
            >
              新建 SSH 目标
            </Button>
          </div>
          <ul className="divide-y">
            {targets.map((target) => {
              const selected =
                target.id === selectedId &&
                target.revision === state.policy.target_revision;
              return (
                <li
                  key={`${target.id}-r${target.revision}`}
                  className="flex flex-wrap items-center gap-2 px-3 py-2 text-sm"
                >
                  <span className="font-medium">{target.name}</span>
                  <code className="text-xs text-muted-foreground">
                    {target.id} r{target.revision}
                  </code>
                  <Badge variant="outline">{backendLabel(target)}</Badge>
                  {!target.enabled ? <Badge variant="destructive">停用</Badge> : null}
                  {selected ? <Badge>当前</Badge> : null}
                  <span className="flex-1" />
                  {!selected ? (
                    <Button
                      size="sm"
                      variant="secondary"
                      disabled={busy !== null || !target.enabled}
                      onClick={() =>
                        void run(`select:${target.id}`, async () => {
                          await selectComputeTarget(revision, target.id);
                          setNotice({
                            kind: "ok",
                            text: `已切换计算策略到 ${target.name} r${target.revision};进行中的运行保持原目标。`,
                          });
                          await reload();
                        })
                      }
                    >
                      选择
                    </Button>
                  ) : null}
                  <Button
                    size="sm"
                    variant="ghost"
                    disabled={busy !== null}
                    onClick={() =>
                      void run(`check:${target.id}`, async () => {
                        const report = await checkComputeTarget(target.id);
                        setCheckReport((prev) => ({
                          ...prev,
                          [target.id]: report,
                        }));
                      })
                    }
                  >
                    检查
                  </Button>
                  <Button
                    size="sm"
                    variant="ghost"
                    disabled={busy !== null}
                    onClick={() => setDraft(draftFromTarget(target))}
                  >
                    编辑
                  </Button>
                </li>
              );
            })}
          </ul>
          {Object.entries(checkReport).map(([id, report]) => (
            <div
              key={id}
              className="border-t px-3 py-2 text-xs text-muted-foreground"
            >
              检查 {id}:
              <Badge
                className="ml-2"
                variant={report.state === "ok" ? "default" : "outline"}
              >
                {report.state}
              </Badge>
              {report.error ? (
                <span className="ml-2">{String(report.error)}</span>
              ) : null}
            </div>
          ))}
        </section>

        {draft ? (
          <TargetForm
            draft={draft}
            busy={busy !== null}
            onCancel={() => setDraft(null)}
            onSave={(next) =>
              void run("save-target", async () => {
                const backend = next.remote
                  ? {
                      type: next.remote.kind,
                      settings: cleanSettings(next.remote.settings),
                    }
                  : { type: "local" };
                await saveComputeTarget(revision, {
                  id: next.id,
                  name: next.name,
                  enabled: next.enabled,
                  backend,
                });
                setDraft(null);
                setNotice({ kind: "ok", text: "目标已保存为新的不可变修订。" });
                await reload();
              })
            }
          />
        ) : null}

        <section className="rounded-lg border">
          <div className="flex items-center justify-between border-b px-3 py-2">
            <span className="text-sm font-medium">数据源</span>
            <Button
              size="sm"
              variant="outline"
              onClick={() => setShowSourceForm((v) => !v)}
            >
              挂载 SSH 数据源
            </Button>
          </div>
          {sources.length ? (
            <ul className="divide-y text-sm">
              {sources.map((source) => (
                <li
                  key={source.id}
                  className="flex flex-wrap items-center gap-2 px-3 py-2"
                >
                  <span className="font-medium">{source.name}</span>
                  <code className="text-xs text-muted-foreground">
                    /data/{source.id}
                  </code>
                  <Badge variant="outline">
                    {source.backend.type === "ssh"
                      ? `SSH · ${source.backend.alias}:${source.backend.root}`
                      : `本地 · ${source.backend.root}`}
                  </Badge>
                  {source.read_only !== false ? (
                    <Badge variant="outline">只读</Badge>
                  ) : null}
                </li>
              ))}
            </ul>
          ) : (
            <p className="px-3 py-2 text-xs text-muted-foreground">
              尚无数据源。SSH 数据源经操作员 OpenSSH 配置只读挂载到
              /data/&lt;id&gt;,与计算目标解耦。
            </p>
          )}
          {showSourceForm ? (
            <div className="grid gap-2 border-t p-3 sm:grid-cols-2">
              <Field label="标识 (id)">
                <input
                  className={inputClass}
                  value={sourceDraft.id}
                  onChange={(e) =>
                    setSourceDraft({ ...sourceDraft, id: e.target.value })
                  }
                />
              </Field>
              <Field label="名称">
                <input
                  className={inputClass}
                  value={sourceDraft.name}
                  onChange={(e) =>
                    setSourceDraft({ ...sourceDraft, name: e.target.value })
                  }
                />
              </Field>
              <Field label="SSH 别名" hint="操作员 ~/.ssh/config 中的主机别名">
                <input
                  className={inputClass}
                  value={sourceDraft.alias}
                  onChange={(e) =>
                    setSourceDraft({ ...sourceDraft, alias: e.target.value })
                  }
                />
              </Field>
              <Field label="远端根目录">
                <input
                  className={inputClass}
                  value={sourceDraft.root}
                  onChange={(e) =>
                    setSourceDraft({ ...sourceDraft, root: e.target.value })
                  }
                />
              </Field>
              <Field label="远端 Python">
                <input
                  className={inputClass}
                  value={sourceDraft.python}
                  onChange={(e) =>
                    setSourceDraft({ ...sourceDraft, python: e.target.value })
                  }
                />
              </Field>
              <div className="flex items-end gap-2">
                <Button
                  size="sm"
                  disabled={busy !== null}
                  onClick={() =>
                    void run("add-source", async () => {
                      await addComputeDataSource(revision, {
                        id: sourceDraft.id,
                        name: sourceDraft.name,
                        backend: {
                          type: "ssh",
                          alias: sourceDraft.alias,
                          root: sourceDraft.root,
                          python: sourceDraft.python,
                        },
                        read_only: true,
                      });
                      setShowSourceForm(false);
                      setNotice({ kind: "ok", text: "数据源已挂载。" });
                      await reload();
                    })
                  }
                >
                  挂载
                </Button>
              </div>
            </div>
          ) : null}
        </section>

        <Separator />

        <section className="rounded-lg border">
          <div className="border-b px-3 py-2 text-sm font-medium">
            运行绑定(按次运行钉定目标)
          </div>
          {runs.length ? (
            <ul className="divide-y text-xs">
              {runs.map((binding) => (
                <li key={binding.id} className="flex gap-2 px-3 py-2">
                  <code className="text-muted-foreground">
                    {binding.target_id} r{binding.target_revision}
                  </code>
                  <span className="flex-1 truncate" title={binding.id}>
                    {binding.id}
                  </span>
                  <code
                    className="text-muted-foreground/70"
                    title={binding.target_sha256}
                  >
                    {binding.target_sha256.slice(0, 12)}
                  </code>
                </li>
              ))}
            </ul>
          ) : (
            <p className="px-3 py-2 text-xs text-muted-foreground">
              暂无运行绑定。
            </p>
          )}
        </section>
      </div>
    </ScrollArea>
  );
}

function TargetForm({
  draft,
  busy,
  onCancel,
  onSave,
}: {
  draft: TargetDraft;
  busy: boolean;
  onCancel: () => void;
  onSave: (draft: TargetDraft) => void;
}) {
  const [current, setCurrent] = useState<TargetDraft>(draft);
  useEffect(() => setCurrent(draft), [draft]);
  const setSlurm = (patch: Partial<SlurmSettings>) =>
    setCurrent({
      ...current,
      remote: {
        kind: current.remote?.kind ?? "slurm",
        settings: { ...(current.remote?.settings ?? emptySlurmDraft()), ...patch },
      },
    });
  const creating = !draft.id;
  return (
    <section className="rounded-lg border p-3">
      <div className="mb-2 text-sm font-medium">
        {creating ? "新建目标" : `编辑目标 ${draft.id}`}
      </div>
      <div className="grid gap-2 sm:grid-cols-2">
        <Field label="标识 (id)" hint="创建后不可变;编辑现有目标时保持一致">
          <input
            className={inputClass}
            value={current.id}
            disabled={!creating}
            onChange={(e) => setCurrent({ ...current, id: e.target.value })}
          />
        </Field>
        <Field label="名称">
          <input
            className={inputClass}
            value={current.name}
            onChange={(e) => setCurrent({ ...current, name: e.target.value })}
          />
        </Field>
        {current.remote ? (
          <>
            <Field label="SSH 主机 (别名)">
              <input
                className={inputClass}
                value={current.remote.settings.ssh_host ?? ""}
                onChange={(e) => setSlurm({ ssh_host: e.target.value })}
              />
            </Field>
            <Field label="SSH 程序">
              <input
                className={inputClass}
                value={current.remote.settings.ssh_command ?? "ssh"}
                onChange={(e) => setSlurm({ ssh_command: e.target.value })}
              />
            </Field>
            <Field label="远端 scratch 根" hint="~/ 前缀展开为远端 home">
              <input
                className={inputClass}
                value={current.remote.settings.scratch_remote_dir ?? ""}
                onChange={(e) =>
                  setSlurm({ scratch_remote_dir: e.target.value })
                }
              />
            </Field>
            {current.remote.kind === "slurm" ? (
              <>
            <Field label="默认分区">
              <input
                className={inputClass}
                value={current.remote.settings.partition ?? ""}
                onChange={(e) => setSlurm({ partition: e.target.value })}
              />
            </Field>
            <Field label="GPU 分区">
              <input
                className={inputClass}
                value={current.remote.settings.gpu_partition ?? ""}
                onChange={(e) => setSlurm({ gpu_partition: e.target.value })}
              />
            </Field>
            <Field label="记账账户 (可选)">
              <input
                className={inputClass}
                value={current.remote.settings.account ?? ""}
                onChange={(e) => setSlurm({ account: e.target.value || null })}
              />
            </Field>
              </>
            ) : null}
            <Field
              label="typed-worker 配置 (本地私有 JSON)"
              hint="钉定集群 worker 可执行文件/源修订;留空表示仅 process 路由"
            >
              <input
                className={inputClass}
                value={current.remote.settings.typed_worker_config ?? ""}
                onChange={(e) =>
                  setSlurm({
                    typed_worker_config: e.target.value || null,
                  })
                }
              />
            </Field>
            {current.remote.kind === "slurm" ? (
              <Field
                label="Apptainer SIF 清单 (本地私有 JSON)"
                hint="容器节点需要;留空则容器任务在远端失败关闭"
              >
                <input
                  className={inputClass}
                  value={current.remote.settings.apptainer_manifest ?? ""}
                  onChange={(e) =>
                    setSlurm({ apptainer_manifest: e.target.value || null })
                  }
                />
              </Field>
            ) : null}
            <Field label="数据传输 SSH 主机 (可选)">
              <input
                className={inputClass}
                value={current.remote.settings.data_ssh_host ?? ""}
                onChange={(e) =>
                  setSlurm({ data_ssh_host: e.target.value || null })
                }
              />
            </Field>
            <Field label="远端 Python">
              <input
                className={inputClass}
                value={current.remote.settings.remote_python ?? "python3"}
                onChange={(e) => setSlurm({ remote_python: e.target.value })}
              />
            </Field>
            <label className="flex items-center gap-2 self-end text-sm">
              <input
                type="checkbox"
                className="size-4"
                checked={current.enabled}
                onChange={(e) =>
                  setCurrent({ ...current, enabled: e.target.checked })
                }
              />
              启用
            </label>
          </>
        ) : null}
      </div>
      <div className="mt-3 flex gap-2">
        <Button
          size="sm"
          disabled={busy || !current.id || !current.name}
          onClick={() => onSave(current)}
        >
          保存(新修订)
        </Button>
        <Button size="sm" variant="ghost" onClick={onCancel}>
          取消
        </Button>
      </div>
    </section>
  );
}

function cleanSettings(settings: SlurmSettings): Record<string, unknown> {
  // 只提交表单里可见的键;其余交给后端 serde 默认值。
  const keys = [
    "enabled",
    "ssh_host",
    "ssh_command",
    "scratch_remote_dir",
    "partition",
    "gpu_partition",
    "gpu_cpus_per_gpu",
    "gpu_qos",
    "account",
    "qos",
    "mem_per_cpu",
    "default_time",
    "typed_worker_config",
    "apptainer_manifest",
    "tools_enabled",
    "data_ssh_host",
    "pi_ssh_host",
    "remote_python",
    "panel_cache_remote_dir",
    "artifact_root",
    "poll_interval_secs",
  ];
  const result: Record<string, unknown> = {};
  for (const key of keys) {
    const value = settings[key];
    if (value !== undefined) result[key] = value;
  }
  return result;
}

function errorText(error: unknown): string {
  if (error instanceof GatewayHttpError) {
    return error.message || `HTTP ${error.status}`;
  }
  return error instanceof Error ? error.message : String(error);
}
