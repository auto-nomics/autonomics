/**
 * 计算目标编辑器:管理 compute registry state(目标/策略/数据源)、检查目标、
 * 查看 worker 兼容性。操作员级操作,经网关 HTTP 完成;每个目标按 id 保存当前配置。
 */
import { useCallback, useEffect, useMemo, useState } from "react";
import { useTranslation } from "react-i18next";
import { GatewayHttpError } from "@/gateway";
import { startEnrollment, useEnrollmentRun } from "./enrollment";
import { EnrollmentProgressPanel } from "./enrollment-progress";
import { Field as FormField, FieldGroup, FieldLabel, FieldDescription } from "@/components/ui/field";
import {
  Dialog,
  DialogClose,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
  DialogTrigger,
} from "@/components/ui/dialog";
import { Textarea } from "@/components/ui/textarea";
import { Input } from "@/components/ui/input";
import {
  checkComputeTarget,
  deleteComputeTarget,
  fetchComputeStatus,
  saveComputeTarget,
  selectComputeTarget,
  type TargetMountSettings,
} from "./api";
import type {
  ComputeBackend,
  ComputeStatus,
  ComputeTarget,
  DataDomain,
  DataSource,
  ComputeTargetCheck,
  SlurmSettings,
} from "./types";
import { Alert, AlertDescription } from "@/components/ui/alert";
import { Button } from "@/components/ui/button";
import { Badge } from "@/components/ui/badge";
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
  dataDomains: DataDomain[];
};

function targetDataDomain(id: string): string {
  const domain = id.trim().toLowerCase().replaceAll("_", "-");
  return /^[a-z][a-z0-9+.-]*$/.test(domain)
    && !["local", "vfs", "file", "http", "https"].includes(domain)
    ? domain
    : `source-${domain}`;
}

function backendLabel(target: ComputeTarget, t: (key: string) => string): string {
  if (target.backend.type === "local") return t("compute.local");
  const s = target.backend.settings;
  const kind = target.backend.type === "ssh" ? "SSH" : "Slurm";
  return `${kind} · ${s.ssh_host ?? "?"}${(s.typed_worker || s.typed_worker_config) ? " · typed-worker" : ""}`;
}

function draftFromTarget(target: ComputeTarget): TargetDraft {
  return {
    id: target.id,
    name: target.name,
    enabled: target.enabled,
    dataDomains: target.data_domains ?? [],
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

const enrollPodmanFields = [
  ["podmanProgram", "field.podmanProgram", true, ""],
  ["podmanSha", "field.podmanSha", true, ""],
  ["pluginsRoot", "field.pluginsRoot", true, ""],
  ["pluginsSha", "field.pluginsSha", true, ""],
  ["imageCatalog", "field.imageCatalog", true, ""],
  ["imageCatalogSha", "field.imageCatalogSha", true, ""],
] as const;

export function ComputeEditorView({ embedded = false }: { embedded?: boolean }) {
  const { t } = useTranslation("hpc");
  const [status, setStatus] = useState<ComputeStatus | null>(null);
  const [loadError, setLoadError] = useState<string | null>(null);
  const [localBusy, setBusy] = useState<string | null>(null);
  const enrollment = useEnrollmentRun();
  const busy = localBusy ?? (enrollment?.status === "running" ? "enroll" : null);
  const [notice, setNotice] = useState<Status>(null);
  const [dialogError, setDialogError] = useState<string | null>(null);
  const [checkReport, setCheckReport] = useState<
    Record<string, ComputeTargetCheck>
  >({});
  const [draft, setDraft] = useState<TargetDraft | null>(null);
  const [deleteTargetId, setDeleteTargetId] = useState<string | null>(null);
  const [enroll, setEnroll] = useState({ backend: "ssh" as "ssh" | "slurm", id: "", name: "", login: "", worker: "", dataAlias: "", dataRoot: "", partition: "", account: "", deploy: "", python: "python3", sshCommand: "", select: true, podmanProgram: "/usr/bin/podman", podmanSha: "", pluginsRoot: "", pluginsSha: "", imageCatalog: "", imageCatalogSha: "", sifManifest: "", sifRuntime: "apptainer" as "apptainer" | "singularity_ce" });
  const enrollmentRoot = enroll.dataRoot.trim() || `~/.autonomics/compute/${enroll.id.trim() || "<目标ID>"}`;
  const renderEnrollmentField = ([key, labelKey, required, hintKey]: readonly [Exclude<keyof typeof enroll, "backend" | "select">, string, boolean, string]) => (
    <FormField key={key}>
      <FieldLabel htmlFor={`enroll-${key}`}>{t(`compute.${labelKey}`)}</FieldLabel>
      <Input id={`enroll-${key}`} required={required} value={enroll[key]}
        pattern={key.endsWith("Sha") ? "[0-9a-fA-F]{64}" : undefined}
        onChange={(event) => setEnroll({ ...enroll, [key]: event.target.value })} />
      {hintKey ? <FieldDescription>{t(`compute.${hintKey}`)}</FieldDescription> : null}
    </FormField>
  );
  const [showEnroll, setShowEnroll] = useState(false);
  const canEnroll = busy === null
    && Boolean(enroll.id.trim() && enroll.login.trim())
    && (!enroll.dataAlias.trim() || Boolean(enroll.dataRoot.trim()))
    && (enroll.backend === "ssh"
      ? enrollPodmanFields.every(([key]) => Boolean(enroll[key].trim()))
      : Boolean(enroll.sifManifest.trim()));

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
  const selectedId = state?.policy.target_id ?? "";

  const run = useCallback(
    async (key: string, action: () => Promise<void>, errorLocation: "page" | "dialog" = "page") => {
      setBusy(key);
      setNotice(null);
      setDialogError(null);
      try {
        await action();
      } catch (error) {
        const message = errorText(error);
        if (errorLocation === "dialog") setDialogError(message);
        else setNotice({ kind: "error", text: message });
      } finally {
        setBusy(null);
      }
    },
    [],
  );

  const targets = useMemo(() => (state?.targets ?? []).filter((target) => !state?.deleted_target_ids?.includes(target.id)), [state]);
  const otherSources = useMemo(() => {
    if (!state) return [];
    const removedPaths = new Set(state.targets
      .filter((target) => target.data_source && state.deleted_target_ids?.includes(target.id))
      .flatMap((target) => target.data_domains?.map((domain) => domain.retained_vfs_prefix) ?? []));
    return state.data_sources.filter((source) => !removedPaths.has(`/data/${source.id}`));
  }, [state]);
  if (loadError && !status) {
    return (
      <div className="p-4 text-sm text-muted-foreground">
        {t("compute.loadFailed", { error: loadError })}
        <Button className="ml-2" size="sm" onClick={() => void reload()}>
          {t("compute.retry")}
        </Button>
      </div>
    );
  }
  if (!status || !state) {
    return <div className="p-4 text-sm text-muted-foreground">{t("compute.loading")}</div>;
  }

  return (
    <div className={embedded ? "" : "min-h-0 flex-1 overflow-auto"}>
      <div className="mx-auto grid max-w-4xl gap-4 p-4 pb-12">
        <header className="flex flex-wrap items-center justify-between gap-2">
          <div>
            <h2 className="text-base font-semibold">{t("compute.title")}</h2>
            <p className="text-xs text-muted-foreground">
              {t("compute.currentTarget", { id: selectedId || t("compute.notSelected") })}
            </p>
          </div>
          <Badge
            variant={
              status.deployment?.source_compatible === true
                ? "default"
                : "destructive"
            }
          >
            {status.deployment?.source_compatible === true
              ? t("compute.sourceCompatible")
              : t("compute.sourceIncompatible")}
          </Badge>
        </header>

        {notice ? (
          <div
            role={notice.kind === "error" ? "alert" : "status"}
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

        {loadError ? (
          <p role="status" className="text-xs text-muted-foreground">
            {t("compute.refreshFailed", { error: loadError })}
          </p>
        ) : null}

        {enrollment ? <EnrollmentProgressPanel run={enrollment} /> : null}

        <section className="rounded-lg border">
          <div className="flex flex-wrap items-center justify-between gap-2 border-b px-3 py-2">
            <span className="shrink-0 text-sm font-medium">{t("compute.targetsSection")}</span>
            <Dialog open={showEnroll} onOpenChange={setShowEnroll}>
              <DialogTrigger render={<Button size="sm" variant="outline" />} disabled={busy !== null}>
                {t("compute.enrollOpen")}
              </DialogTrigger>
              <DialogContent className="max-h-[calc(100dvh-2rem)] overflow-y-auto sm:max-w-2xl">
                <DialogHeader className="pr-6">
                  <DialogTitle>{t("compute.enrollTitle")}</DialogTitle>
                  <DialogDescription>{t("compute.enrollDescription")}</DialogDescription>
                </DialogHeader>
                <form onSubmit={(event) => {
                  event.preventDefault();
                  if (!canEnroll) return;
                  setShowEnroll(false);
                  void run("enroll", async () => {
                    try {
                      await startEnrollment({
                        backend: enroll.backend, id: enroll.id.trim(), login_alias: enroll.login.trim(),
                        container_runtime: enroll.backend === "ssh" ? {
                          type: "podman", plugins_root: enroll.pluginsRoot.trim(), plugins_sha256: enroll.pluginsSha.trim(),
                          podman: { executable: enroll.podmanProgram.trim(), executable_sha256: enroll.podmanSha.trim(),
                            image_catalog: enroll.imageCatalog.trim(), image_catalog_sha256: enroll.imageCatalogSha.trim() },
                        } : { type: "sif", image_manifest: enroll.sifManifest.trim(), runtime: enroll.sifRuntime },
                        worker_executable: enroll.worker.trim() || undefined, name: enroll.name.trim() || undefined,
                        data_alias: enroll.dataAlias.trim() || undefined, storage_root: enroll.dataRoot.trim() || undefined,
                        partition: enroll.backend === "slurm" ? enroll.partition.trim() || undefined : undefined, account: enroll.backend === "slurm" ? enroll.account.trim() || undefined : undefined,
                        select: enroll.select,
                        deploy_worker: enroll.deploy.trim() || undefined, python: enroll.python.trim() || undefined,
                        ssh_command: enroll.sshCommand.trim() || undefined,
                      });
                      setNotice({ kind: "ok", text: t("compute.enrollSuccess") });
                    } finally { await reload(); }
                  });
                }}>
                  <fieldset disabled={busy !== null}>
                    <legend className="sr-only">{t("compute.enrollLegend")}</legend>
                    <FieldGroup className="grid gap-3 sm:grid-cols-2">
                      <FormField>
                        <FieldLabel htmlFor="enroll-backend">{t("compute.backend")}</FieldLabel>
                        <select id="enroll-backend" className={inputClass} value={enroll.backend}
                          onChange={event => setEnroll({ ...enroll, backend: event.target.value as "ssh" | "slurm" })}>
                          <option value="ssh">{t("compute.backendSsh")}</option>
                          <option value="slurm">{t("compute.backendSlurm")}</option>
                        </select>
                      </FormField>
                      {([
                        ["id", "field.id", true, ""], ["name", "field.name", false, ""],
                        ["login", "field.login", true, ""],
                        ["partition", "field.partition", false, ""], ["account", "field.account", false, ""],
                      ] as const).filter(([key]) => enroll.backend === "slurm" || (key !== "partition" && key !== "account")).map(renderEnrollmentField)}
                      <fieldset className="min-w-0 rounded-lg border px-3 pb-3 sm:col-span-2">
                        <legend className="px-1 text-sm font-medium">{t("compute.runtime")}</legend>
                        <FieldGroup className="grid gap-3 sm:grid-cols-2">
                          {enroll.backend === "ssh" ? enrollPodmanFields.map(renderEnrollmentField) : (
                            <>
                              <Field label={t("compute.sifRuntime")}>
                                <select className={inputClass} value={enroll.sifRuntime}
                                  onChange={event => setEnroll({ ...enroll, sifRuntime: event.target.value as typeof enroll.sifRuntime })}>
                                  <option value="apptainer">Apptainer</option>
                                  <option value="singularity_ce">SingularityCE</option>
                                </select>
                              </Field>
                              {renderEnrollmentField(["sifManifest", "field.sifManifest", true, "hint.sifManifest"])}
                            </>
                          )}
                        </FieldGroup>
                      </fieldset>
                      <fieldset className="min-w-0 rounded-lg border px-3 pb-3 sm:col-span-2">
                        <legend className="px-1 text-sm font-medium">{t("compute.mountSection")}</legend>
                        <p className="mb-3 break-words text-xs text-muted-foreground">
                          {t("compute.mountRootPrefix")} <code>{enroll.id.trim() ? targetDataDomain(enroll.id) : t("compute.domainPlaceholder")}://</code> {t("compute.mountRootSuffix")}
                        </p>
                        <FieldGroup className="grid gap-3 sm:grid-cols-2">
                          {([
                            ["dataRoot", "field.dataRoot", false, "hint.dataRoot"],
                            ["dataAlias", "field.dataAlias", false, "hint.dataAlias"],
                          ] as const).map(renderEnrollmentField)}
                        </FieldGroup>
                        <details className="mt-3">
                          <summary className="cursor-pointer text-sm">{t("compute.viewLayout")}</summary>
                          <div className="mt-3">
                            <PathDetails entries={[
                              [t("compute.path.remoteStorageRoot"), enrollmentRoot],
                              [t("compute.path.dataRootDir"), `${enrollmentRoot}/data`],
                              [t("compute.path.remoteDagTasks"), `${enrollmentRoot}/dag-tasks`],
                              ...unifiedStorageDirectories(enrollmentRoot, t),
                              [t("compute.path.remotePanels"), `${enrollmentRoot}/panels`],
                            ]} />
                          </div>
                        </details>
                      </fieldset>
                      <details className="sm:col-span-2">
                        <summary className="cursor-pointer text-sm">{t("compute.advanced")}</summary>
                        <FieldGroup className="mt-3 grid gap-3 sm:grid-cols-2">
                          {([
                            ["worker", "field.worker", false, "hint.worker"],
                            ["deploy", "field.deploy", false, "hint.deploy"],
                            ["python", "field.python", false, "hint.python"],
                            ["sshCommand", "field.sshCommand", false, "hint.sshCommand"],
                          ] as const).map(renderEnrollmentField)}
                        </FieldGroup>
                      </details>
                      <FormField orientation="horizontal">
                        <input id="enroll-select" type="checkbox" checked={enroll.select}
                          onChange={(event) => setEnroll({ ...enroll, select: event.target.checked })} />
                        <FieldLabel htmlFor="enroll-select">{t("compute.selectAfter")}</FieldLabel>
                      </FormField>
                    </FieldGroup>
                  </fieldset>
                  <DialogFooter className="mt-4">
                    <DialogClose render={<Button type="button" size="sm" variant="outline" />}>
                      {t("compute.cancel")}
                    </DialogClose>
                    <Button type="submit" size="sm" disabled={!canEnroll}>
                      {!enroll.worker.trim() || enroll.deploy.trim() ? t("compute.submitInstall") : t("compute.submitValidate")}
                    </Button>
                  </DialogFooter>
                </form>
              </DialogContent>
            </Dialog>
          </div>
          <ul className="divide-y">
            {targets.length === 0 ? (
              <li className="px-3 py-2 text-xs text-muted-foreground">{t("compute.empty")}</li>
            ) : null}
            {targets.map((target) => {
              const selected = target.id === selectedId;
              const editing = draft?.id === target.id;
              const report = checkReport[target.id];
              return (
                <li
                  key={target.id}
                  className="flex flex-wrap items-center gap-2 px-3 py-2 text-sm"
                >
                  <span className="font-medium">{target.name}</span>
                  <code className="text-xs text-muted-foreground">
                    {target.id}
                  </code>
                  <Badge variant="outline">{backendLabel(target, t)}</Badge>
                  {!target.enabled ? <Badge variant="destructive">{t("compute.disabledBadge")}</Badge> : null}
                  {selected ? <Badge>{t("compute.currentBadge")}</Badge> : null}
                  <span className="flex-1" />
                  {!selected ? (
                    <Button
                      size="sm"
                      variant="secondary"
                      disabled={busy !== null || !target.enabled}
                      onClick={() =>
                        void run(`select:${target.id}`, async () => {
                          await selectComputeTarget(target.id);
                          setNotice({
                            kind: "ok",
                            text: t("compute.switched", { name: target.name }),
                          });
                          await reload();
                        })
                      }
                    >
                      {t("compute.selectButton")}
                    </Button>
                  ) : null}
                  <Button
                    size="sm"
                    variant="ghost"
                    disabled={busy !== null}
                    onClick={() =>
                      void run(`check:${target.id}`, async () => {
                        setCheckReport((previous) => {
                          const next = { ...previous };
                          delete next[target.id];
                          return next;
                        });
                        const report = await checkComputeTarget(target.id);
                        setCheckReport((prev) => ({
                          ...prev,
                          [target.id]: report,
                        }));
                      })
                    }
                  >
                    {t("compute.checkButton")}
                  </Button>
                  <Dialog open={editing} onOpenChange={(open) => {
                    if (busy !== null) return;
                    setDialogError(null);
                    setDraft(open ? draftFromTarget(target) : null);
                  }}>
                    <DialogTrigger render={<Button size="sm" variant="ghost" />} disabled={busy !== null}>
                      {t("compute.editButton")}
                    </DialogTrigger>
                    <DialogContent className="max-h-[calc(100dvh-2rem)] overflow-y-auto sm:max-w-2xl"
                      showCloseButton={busy === null}>
                      <DialogHeader className="pr-6">
                        <DialogTitle className="break-words">{t("compute.editTitle", { id: target.id })}</DialogTitle>
                        <DialogDescription>{t("compute.editDescription")}</DialogDescription>
                      </DialogHeader>
                      <ComputeDialogError message={dialogError} />
                      {editing && (
                        <TargetForm
                          key={draft.id}
                          draft={draft}
                          dataSource={target.data_source}
                          retainedSources={target.retained_data_sources}
                          busy={busy !== null}
                          saving={busy === "save-target"}
                          onSave={(next, mount) =>
                            void run("save-target", async () => {
                              const backend: ComputeBackend = next.remote
                                ? { type: next.remote.kind, settings: next.remote.settings }
                                : { type: "local" };
                              await saveComputeTarget({
                                id: next.id,
                                name: next.name,
                                enabled: next.enabled,
                                backend,
                                data_domains: next.dataDomains,
                              }, mount);
                              setCheckReport((previous) => {
                                const remaining = { ...previous };
                                delete remaining[next.id];
                                return remaining;
                              });
                              setDraft(null);
                              setNotice({ kind: "ok", text: t("compute.saved") });
                              await reload();
                            }, "dialog")
                          }
                        />
                      )}
                    </DialogContent>
                  </Dialog>
                  {target.id !== "local" && (
                    <Dialog open={deleteTargetId === target.id} onOpenChange={(open) => {
                      if (busy !== null) return;
                      setDialogError(null);
                      setDeleteTargetId(open ? target.id : null);
                    }}>
                      <DialogTrigger
                        render={<Button size="sm" variant="ghost" />}
                        disabled={busy !== null || selected}
                        title={selected ? t("compute.deleteTitleHint") : undefined}
                      >
                        {t("compute.deleteButton")}
                      </DialogTrigger>
                      <DialogContent role="alertdialog" showCloseButton={false}>
                        <DialogHeader className="min-w-0">
                          <DialogTitle className="break-words">{t("compute.deleteTitle", { name: target.name })}</DialogTitle>
                          <DialogDescription>{t("compute.deleteDescription")}</DialogDescription>
                        </DialogHeader>
                        <ComputeDialogError message={dialogError} />
                        <DialogFooter>
                          <DialogClose render={<Button size="sm" variant="outline" />} disabled={busy !== null}>
                            {t("compute.cancel")}
                          </DialogClose>
                          <Button
                            size="sm"
                            variant="destructive"
                            disabled={busy !== null || selected}
                            onClick={() => void run("delete-target", async () => {
                              const updated = await deleteComputeTarget(target.id);
                              setDeleteTargetId(null);
                              setStatus((previous) => previous && { ...previous, state: updated });
                              setNotice({ kind: "ok", text: t("compute.deleted", { name: target.name }) });
                              window.dispatchEvent(new Event("autonomics:compute-changed"));
                            }, "dialog")}
                          >
                            {busy === "delete-target" ? t("compute.deleting") : t("compute.confirmDelete")}
                          </Button>
                        </DialogFooter>
                      </DialogContent>
                    </Dialog>
                  )}
                  {report ? (
                    <div role="status" className="basis-full break-words text-xs text-muted-foreground">
                      <Badge variant={report.configuration_valid === true ? "default" : "outline"}>
                        {report.configuration_valid === true
                          ? target.backend.type === "local" ? t("compute.checkPassedLocal") : t("compute.checkPassedRemote")
                          : t("compute.checkFailed")}
                      </Badge>
                      {report.error ? <span className="ml-2">{String(report.error)}</span> : null}
                    </div>
                  ) : null}
                </li>
              );
            })}
          </ul>
        </section>

        {otherSources.length > 0 ? (
          <section className="rounded-lg border" aria-label={t("compute.otherSources")}>
            <h3 className="border-b px-3 py-2 text-sm font-medium">{t("compute.otherSources")}</h3>
            <ul className="divide-y text-sm">
              {otherSources.map((source) => (
                <li key={source.id} className="flex flex-wrap items-center gap-2 px-3 py-2">
                  <span className="font-medium">{source.name}</span>
                  <code className="break-all text-xs text-muted-foreground">{source.domain ? `${source.domain}://` : `/data/${source.id}`}</code>
                  <Badge variant="outline">{source.read_only !== false ? t("compute.readOnly") : t("compute.writable")}</Badge>
                </li>
              ))}
            </ul>
          </section>
        ) : null}

      </div>
    </div>
  );
}

const sshWorkerFields = [
  ["worker", "executable", "field.workerExecutable"],
  ["worker", "executable_sha256", "field.workerSha"],
  ["worker", "expected_source_revision", "field.workerRevision"],
  ["worker", "plugins_root", "field.pluginsRoot"],
  ["worker", "plugins_sha256", "field.pluginsSha"],
  ["podman", "executable", "field.podmanProgram"],
  ["podman", "executable_sha256", "field.podmanSha"],
  ["podman", "image_catalog", "field.imageCatalog"],
  ["podman", "image_catalog_sha256", "field.imageCatalogSha"],
] as const;

function parseProfileObject(text: string) {
  try {
    const value = JSON.parse(text || "{}");
    return value && typeof value === "object" && !Array.isArray(value) ? value : null;
  } catch {
    return null;
  }
}

function TargetForm({
  draft,
  dataSource,
  retainedSources = [],
  busy,
  saving,
  onSave,
}: {
  draft: TargetDraft;
  dataSource?: DataSource;
  retainedSources?: DataSource[];
  busy: boolean;
  saving: boolean;
  onSave: (draft: TargetDraft, mount?: TargetMountSettings) => void;
}) {
  const { t } = useTranslation("hpc");
  const [current, setCurrent] = useState<TargetDraft>(draft);
  const [workerText, setWorkerText] = useState(() =>
    draft.remote?.settings.typed_worker ? JSON.stringify(draft.remote.settings.typed_worker, null, 2) : "",
  );
  const [workerTextError, setWorkerTextError] = useState("");
  const [editMount, setEditMount] = useState(false);
  const [mount, setMount] = useState<TargetMountSettings>(() => {
    const backend = dataSource?.backend;
    return backend?.type === "ssh"
      ? { alias: backend.alias, root: backend.root, python: backend.python }
      : {
        alias: draft.remote?.settings.data_ssh_host || draft.remote?.settings.ssh_host || "",
        root: "",
        python: draft.remote?.settings.remote_python || "python3",
      };
  });
  const artifactMount = current.dataDomains[0]?.retained_vfs_prefix;
  const previousMounts = retainedSources.filter(source => `/data/${source.id}` !== artifactMount);
  const mountSettings = { alias: mount.alias.trim(), root: mount.root.trim(), python: mount.python.trim() };
  const validMount = !editMount || Object.values(mountSettings).every(Boolean);
  const setRemoteSettings = (patch: Partial<SlurmSettings>) => {
    setCurrent((previous) => {
      if (!previous.remote) return previous;
      return {
        ...previous,
        remote: {
          ...previous.remote,
          settings: { ...previous.remote.settings, ...patch },
        },
      };
    });
  };
  const isSsh = current.remote?.kind === "ssh";
  const profile = isSsh ? parseProfileObject(workerText) : null;
  const workerEditor = (
    <FormField className="sm:col-span-2" data-invalid={Boolean(workerTextError)}>
      <FieldLabel htmlFor="compute-typed-worker">{t("compute.workerConfigLabel")}</FieldLabel>
      <Textarea
        id="compute-typed-worker"
        className="min-h-28 font-mono"
        aria-invalid={Boolean(workerTextError)}
        aria-describedby="compute-typed-worker-description"
        value={workerText}
        onChange={(event) => {
          setWorkerText(event.target.value);
          setWorkerTextError("");
        }}
      />
      <FieldDescription id="compute-typed-worker-description">
        {workerTextError || t("compute.workerConfigHint")}
      </FieldDescription>
    </FormField>
  );
  const name = current.name.trim();
  const valid = Boolean(name && (!current.remote || current.remote.settings.ssh_host?.trim()));

  return (
    <form onSubmit={(event) => {
      event.preventDefault();
      if (!valid || !validMount || busy) return;
      if (isSsh && !profile) {
        setWorkerTextError(t("compute.invalidJson"));
        return;
      }
      let next = { ...current, name };
      if (current.remote) {
        const original = current.remote.settings.typed_worker;
        if (workerText.trim() || original) {
          try {
            const worker = workerText.trim() ? JSON.parse(workerText) : null;
            if (worker !== null && (typeof worker !== "object" || Array.isArray(worker))) {
              throw new Error("invalid profile");
            }
            next = {
              ...next,
              remote: {
                ...current.remote,
                settings: { ...current.remote.settings, typed_worker: worker, typed_worker_config: null },
              },
            };
          } catch {
            setWorkerTextError(t("compute.invalidJson"));
            return;
          }
        }
      }
      onSave(next, editMount ? mountSettings : undefined);
    }}>
      <fieldset disabled={busy} className="grid min-w-0 gap-3 sm:grid-cols-2">
        <legend className="sr-only">{t("compute.formLegend")}</legend>
        <Field label={t("compute.field.name")}>
          <Input
            required
            value={current.name}
            onChange={(e) => setCurrent({ ...current, name: e.target.value })}
          />
        </Field>
        <label className="flex items-center gap-2 self-end text-sm">
          <input
            type="checkbox"
            className="size-4"
            checked={current.enabled}
            onChange={(e) => setCurrent({ ...current, enabled: e.target.checked })}
          />
          {t("compute.enabled")}
        </label>
        {current.remote ? (
          <>
            <Field label={t("compute.field.sshHost")}>
              <input
                className={inputClass}
                required
                value={current.remote.settings.ssh_host ?? ""}
                onChange={(e) => setRemoteSettings({ ssh_host: e.target.value })}
              />
            </Field>
            <Field label={t("compute.field.sshProgram")}>
              <input
                className={inputClass}
                value={current.remote.settings.ssh_command ?? "ssh"}
                onChange={(e) => setRemoteSettings({ ssh_command: e.target.value })}
              />
            </Field>
            {current.remote.kind === "slurm" ? (
              <>
                <Field label={t("compute.field.defaultPartition")}>
                  <input
                    className={inputClass}
                    value={current.remote.settings.partition ?? ""}
                    onChange={(e) => setRemoteSettings({ partition: e.target.value })}
                  />
                </Field>
                <Field label={t("compute.field.gpuPartition")}>
                  <input
                    className={inputClass}
                    value={current.remote.settings.gpu_partition ?? ""}
                    onChange={(e) => setRemoteSettings({ gpu_partition: e.target.value })}
                  />
                </Field>
                <Field label={t("compute.field.accountField")}>
                  <input
                    className={inputClass}
                    value={current.remote.settings.account ?? ""}
                    onChange={(e) => setRemoteSettings({ account: e.target.value || null })}
                  />
                </Field>
              </>
            ) : null}
            {isSsh && profile && sshWorkerFields.map(([section, key, label]) => {
              const worker = profile.worker ?? {};
              const values = section === "worker" ? worker : worker.podman ?? {};
              return (
                <Field key={label} label={t(`compute.${label}`)}>
                  <Input required value={values[key] ?? ""}
                    pattern={key.endsWith("sha256") ? "[0-9a-fA-F]{64}" : undefined}
                    onChange={(event) => {
                      const updated = { ...values, [key]: event.target.value };
                      setWorkerText(JSON.stringify({ ...profile, worker: section === "worker"
                        ? updated : { ...worker, podman: updated } }, null, 2));
                      setWorkerTextError("");
                    }}
                  />
                </Field>
              );
            })}
            {isSsh ? (
              <details className="sm:col-span-2" open={!profile || workerTextError ? true : undefined}>
                <summary>{t("compute.advancedWorker")}</summary>
                {workerEditor}
              </details>
            ) : workerEditor}
            {current.remote.kind === "slurm" && (
              <>
                <Field label={t("compute.runtime")}>
                  <select
                    className={inputClass}
                    value={current.remote.settings.sif_runtime ?? "apptainer"}
                    onChange={(event) => setRemoteSettings({
                      sif_runtime: event.target.value as SlurmSettings["sif_runtime"],
                    })}
                  >
                    <option value="apptainer">Apptainer</option>
                    <option value="singularity_ce">SingularityCE</option>
                  </select>
                </Field>
                <Field label={t("compute.field.sifManifestJson")} hint={t("compute.hint.sifManifestJson")}>
                  <Input
                    required
                    value={current.remote.settings.apptainer_manifest ?? ""}
                    onChange={(event) => setRemoteSettings({ apptainer_manifest: event.target.value || null })}
                  />
                </Field>
                {current.remote.settings.sif_runtime === "singularity_ce" && (
                  <Field label={t("compute.field.singularityExecutable")} hint={t("compute.hint.singularityExecutable")}>
                    <Input
                      value={current.remote.settings.singularity_executable ?? ""}
                      onChange={(event) => setRemoteSettings({ singularity_executable: event.target.value || null })}
                    />
                  </Field>
                )}
              </>
            )}
            <Field label={t("compute.field.dataSshHost")}>
              <input
                className={inputClass}
                value={current.remote.settings.data_ssh_host ?? ""}
                onChange={(e) =>
                  setRemoteSettings({ data_ssh_host: e.target.value || null })
                }
              />
            </Field>
            <Field label={t("compute.field.remotePython")}>
              <input
                className={inputClass}
                value={current.remote.settings.remote_python ?? "python3"}
                onChange={(e) => setRemoteSettings({ remote_python: e.target.value })}
              />
            </Field>
          </>
        ) : null}
      </fieldset>
      {current.remote && (
        <section aria-label={t("compute.storagePaths")} className="mt-4 grid min-w-0 gap-3 rounded-lg border p-3">
          <h4 className="text-sm font-medium">{t("compute.storagePaths")}</h4>
          <TargetStoragePaths targetId={current.id} settings={current.remote.settings} domains={current.dataDomains} />
          {!current.remote.settings.storage_root_v1 && (
            <Field label={t("compute.field.scratchDir")} hint={t("compute.hint.scratchDir")}>
              <Input
                disabled={busy}
                value={current.remote.settings.scratch_remote_dir ?? ""}
                onChange={(event) => setRemoteSettings({ scratch_remote_dir: event.target.value })}
              />
            </Field>
          )}
        </section>
      )}
      {dataSource || current.remote ? (
        <section aria-label={t("compute.mountSection")} className="mt-4 grid gap-3 rounded-lg border p-3">
          <div className="flex flex-wrap items-center justify-between gap-2">
            <h4 className="text-sm font-medium">{t("compute.mountSection")}</h4>
            {current.remote && !current.remote.settings.storage_root_v1 && (
              <Button type="button" size="sm" variant="outline" disabled={busy} onClick={() => setEditMount(!editMount)}>
                {editMount ? t("compute.mountEditCancel") : dataSource ? t("compute.mountEdit") : t("compute.mountConfigure")}
              </Button>
            )}
          </div>
          {editMount ? (
            <fieldset disabled={busy}>
              <legend className="sr-only">{t("compute.mountLegend")}</legend>
              <p className="mb-3 text-xs text-muted-foreground">{t("compute.mountNote")}</p>
              <FieldGroup className="grid gap-3 sm:grid-cols-2">
                {([
                  ["alias", "field.mountAlias"],
                  ["root", "field.mountRoot"],
                  ["python", "field.mountPython"],
                ] as const).map(([key, labelKey]) => (
                  <FormField key={key}>
                    <FieldLabel htmlFor={`target-mount-${key}`}>{t(`compute.${labelKey}`)}</FieldLabel>
                    <Input id={`target-mount-${key}`} required value={mount[key]}
                      onChange={(event) => setMount({ ...mount, [key]: event.target.value })} />
                  </FormField>
                ))}
              </FieldGroup>
            </fieldset>
          ) : <TargetDataSource source={dataSource} domain={current.dataDomains[0]} />}
          {previousMounts.length > 0 && (
            <div className="grid gap-1">
              <span className="text-xs text-muted-foreground">{t("compute.historicalMounts")}</span>
              <ul className="text-sm">
                {previousMounts.map(source => <li className="break-all" key={source.id}>/data/{source.id}</li>)}
              </ul>
            </div>
          )}
        </section>
      ) : null}
      <DialogFooter className="mt-4">
        <DialogClose render={<Button type="button" size="sm" variant="outline" />} disabled={busy}>
          {t("compute.cancel")}
        </DialogClose>
        <Button type="submit" size="sm" disabled={busy || !valid || !validMount}>
          {saving ? t("compute.saving") : t("compute.save")}
        </Button>
      </DialogFooter>
    </form>
  );
}

function PathDetails({ entries }: { entries: [string, string][] }) {
  return (
    <dl className="grid min-w-0 gap-3 sm:grid-cols-2">
      {entries.map(([label, value]) => (
        <div key={label} className="min-w-0">
          <dt className="text-xs text-muted-foreground">{label}</dt>
          <dd className="break-all text-sm">{value}</dd>
        </div>
      ))}
    </dl>
  );
}

function unifiedStorageDirectories(root: string, t: (key: string) => string): [string, string][] {
  return [
    [t("compute.path.remoteWork"), `${root}/work`],
    [t("compute.path.sifImages"), `${root}/images/sif`],
    [t("compute.path.sifRuntimeWork"), `${root}/runtime/apptainer`],
  ];
}

function TargetStoragePaths({
  targetId,
  settings,
  domains,
}: {
  targetId: string;
  settings: SlurmSettings;
  domains: DataDomain[];
}) {
  const { t } = useTranslation("hpc");
  const root = settings.storage_root_v1;
  const domain = domains[0];
  const entries: [string, string][] = [];
  if (root) entries.push([t("compute.path.remoteStorageRoot"), root]);
  const includesRetained = domain?.retained_remote_root && domain.retained_remote_root === settings.scratch_remote_dir;
  entries.push([
    includesRetained ? t("compute.path.remoteDagTasksRetained") : t("compute.path.remoteDagTasks"),
    settings.scratch_remote_dir || t("compute.unregistered"),
  ]);
  if (domain?.retained_remote_root && domain.retained_remote_root !== settings.scratch_remote_dir) {
    entries.push([t("compute.path.retainedArtifacts"), domain.retained_remote_root]);
  }
  if (domain?.retained_vfs_prefix) {
    entries.push([t("compute.path.artifactsVfs"), domain.retained_vfs_prefix]);
  }
  if (root) entries.push(...unifiedStorageDirectories(root, t));
  const localTargetRoot = `状态目录/compute/targets/${targetId}`;
  const legacy = targetId === "legacy-slurm";
  entries.push(
    [t("compute.path.remotePanels"), settings.panel_cache_remote_dir || t("compute.unregistered")],
    [t("compute.path.gatewayTasks"), legacy ? "状态目录/slurm-tasks" : `${localTargetRoot}/tasks`],
    [t("compute.path.gatewayArtifacts"), settings.artifact_root || (legacy
      ? "状态目录/slurm-artifacts"
      : `${localTargetRoot}/artifacts`)],
  );
  return <PathDetails entries={entries} />;
}

function TargetDataSource({ source, domain }: { source?: DataSource; domain?: DataDomain }) {
  const { t } = useTranslation("hpc");
  const details: [string, string][] = [];
  if (source) {
    details.push([t("compute.vfsPath"), domain?.vfs_prefix ?? `/data/${source.id}`]);
    if (source.domain) details.push([t("compute.vfsDomain"), `${source.domain}://`]);
    details.push(
      [t("compute.accessMode"), source.read_only !== false ? t("compute.readOnly") : t("compute.writable")],
      [t("compute.path.dataRootDir"), source.backend.root],
    );
    if (source.backend.type === "ssh") {
      details.push([t("compute.field.dataAlias"), source.backend.alias], [t("compute.field.remotePython"), source.backend.python]);
    }
  } else if (domain) {
    details.push(
      [t("compute.vfsPath"), domain.vfs_prefix],
      [t("compute.path.dataRootDir"), domain.remote_root],
      [t("compute.field.dataAlias"), domain.ssh_aliases.join("、")],
    );
  }
  return (
    <>
      {source ? (
        <>
          <p className="break-words text-xs text-muted-foreground">{source.name}</p>
          <PathDetails entries={details} />
        </>
      ) : domain ? <PathDetails entries={details} /> : (
        <p className="text-xs text-muted-foreground">{t("compute.noMount")}</p>
      )}
    </>
  );
}

function ComputeDialogError({ message }: { message: string | null }) {
  if (!message) return null;
  return (
    <Alert variant="destructive">
      <AlertDescription className="break-words">{message}</AlertDescription>
    </Alert>
  );
}

function errorText(error: unknown): string {
  if (error instanceof GatewayHttpError) {
    return error.message || `HTTP ${error.status}`;
  }
  return error instanceof Error ? error.message : String(error);
}
