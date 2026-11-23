import { useEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { Check, Circle, CircleAlert, LoaderCircle } from "lucide-react";
import { Card, CardHeader, CardTitle, CardDescription, CardContent, CardFooter } from "@/components/ui/card";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Progress, ProgressLabel, ProgressValue } from "@/components/ui/progress";
import { Alert, AlertTitle, AlertDescription } from "@/components/ui/alert";
import { cn } from "@/lib/utils";
import { clearEnrollment, type EnrollmentRun } from "./enrollment";

const stageKeys: Record<string, string> = {
  validate: "validate", connection: "connection", authenticate: "authenticate",
  prepare_directory: "prepareDirectory", check_python: "checkPython",
  upload: "upload", commit_upload: "commitUpload", chmod: "chmod",
  probe_worker: "probeWorker", worker_profile: "workerProfile",
  remote_home: "remoteHome", prepare_data_directory: "prepareDataDirectory",
  data_source: "dataSource", save_target: "saveTarget", select_target: "selectTarget",
};
const bytes = (value: number) => `${(value / 1024 / 1024).toFixed(1)} MB`;

export function EnrollmentProgressPanel({ run }: { run: EnrollmentRun }) {
  const { t } = useTranslation("hpc");
  const panel = useRef<HTMLDivElement>(null);
  useEffect(() => {
    panel.current?.scrollIntoView({ behavior: window.matchMedia("(prefers-reduced-motion: reduce)").matches ? "auto" : "smooth", block: "nearest" });
  }, [run.startedAt]);
  const [now, setNow] = useState(Date.now());
  useEffect(() => {
    setNow(Date.now());
    if (run.status !== "running") return;
    const timer = window.setInterval(() => setNow(Date.now()), 1000);
    return () => window.clearInterval(timer);
  }, [run.startedAt, run.status]);
  const elapsed = Math.max(0, Math.floor(((run.endedAt ?? now) - run.startedAt) / 1000));
  const waiting = run.status === "running" && now - (run.events.at(-1)?.at ?? run.startedAt) >= 15_000;
  const currentIndex = Math.max(0, run.steps.indexOf(run.stage));
  const percent = run.total && run.uploaded !== undefined ? Math.min(100, Math.floor(run.uploaded / run.total * 100)) : null;
  const stageLabel = (stage: string) => (stageKeys[stage] ? t(`enrollment.stage.${stageKeys[stage]}`) : stage);
  const currentLabel = stageLabel(run.stage);
  return (
    <Card ref={panel} size="sm" role="region" aria-label={t("enrollment.panelAria")} data-enrollment-state={run.status}>
      <CardHeader>
        <CardTitle>{t("enrollment.title", { id: run.targetId })}</CardTitle>
        <CardDescription>{t("enrollment.elapsed", { min: Math.floor(elapsed / 60), sec: elapsed % 60 })}</CardDescription>
      </CardHeader>
      <CardContent className="grid gap-4">
        <div role="status" aria-live="polite" className="flex flex-wrap items-center gap-2">
          <Badge variant={run.status === "error" || run.status === "unknown" ? "destructive" : "secondary"}>
            {run.status === "success" ? t("enrollment.statusSuccess") : run.status === "error" ? t("enrollment.statusError") : run.status === "unknown" ? t("enrollment.statusUnknown") : t("enrollment.statusRunning")}
          </Badge>
          <span className="text-sm">{run.status === "success" ? t("enrollment.saved") : currentLabel}</span>
          {waiting ? <span className="text-xs text-muted-foreground">{t("enrollment.waiting")}</span> : null}
        </div>
        {percent !== null ? (
          <div className="grid gap-1">
            <Progress value={percent} aria-label={t("enrollment.uploadAria")}>
              <ProgressLabel>{t("enrollment.uploadLabel")}</ProgressLabel>
              <ProgressValue>{() => `${percent}%`}</ProgressValue>
            </Progress>
            <p className="text-xs text-muted-foreground tabular-nums">
              {bytes(run.uploaded ?? 0)} / {bytes(run.total!)}
              {percent === 100 && run.status === "running" ? t("enrollment.uploadDoneSuffix") : ""}
            </p>
          </div>
        ) : null}
        <ol aria-label={t("enrollment.stepsAria")} className="grid gap-2 sm:grid-cols-2">
          {run.steps.map((stage, index) => {
            const done = run.status === "success" || index < currentIndex;
            const active = index === currentIndex && run.status !== "success";
            const failed = active && (run.status === "error" || run.status === "unknown");
            const Icon = done ? Check : failed ? CircleAlert : active ? LoaderCircle : Circle;
            return (
              <li key={stage} aria-current={active ? "step" : undefined}
                className={cn("flex items-center gap-2 text-xs", !active && !done && "text-muted-foreground", failed && "text-destructive")}>
                <Icon aria-hidden className={cn("size-4 shrink-0", active && !failed && "motion-safe:animate-spin")} />
                <span>{stageLabel(stage)}</span>
                <span className="sr-only">{done ? t("enrollment.stepDone") : failed ? t("enrollment.stepFailed") : active ? t("enrollment.stepActive") : t("enrollment.stepPending")}</span>
              </li>
            );
          })}
        </ol>
        {run.error ? (
          <Alert variant="destructive">
            <CircleAlert />
            <AlertTitle>{run.status === "unknown" ? t("enrollment.connectionLost") : t("enrollment.stageIncomplete", { stage: currentLabel })}</AlertTitle>
            <AlertDescription className="whitespace-pre-wrap break-words">{run.error}</AlertDescription>
          </Alert>
        ) : null}
        <details className="text-xs">
          <summary className="cursor-pointer text-muted-foreground">{t("enrollment.logSummary", { count: run.events.length })}</summary>
          <pre role="log" aria-label={t("enrollment.logAria")} className="mt-2 max-h-64 overflow-auto whitespace-pre-wrap break-words rounded-md bg-muted p-3 font-mono">
            {run.events.map((event, index) => <span key={index} className="block">
              [{Math.max(0, Math.floor((event.at - run.startedAt) / 1000))}s] [{stageLabel(event.stage)}] {event.message}
            </span>)}
          </pre>
        </details>
      </CardContent>
      <CardFooter className="flex flex-wrap justify-between gap-2">
        <p className="text-xs text-muted-foreground">{run.status === "running"
          ? t("enrollment.footerRunning")
          : run.status === "unknown" ? t("enrollment.footerUnknown") : run.status === "error" ? t("enrollment.footerError") : t("enrollment.footerDone")}</p>
        {run.status !== "running" ? <Button size="sm" variant="ghost" onClick={clearEnrollment}>{t("enrollment.collapse")}</Button> : null}
      </CardFooter>
    </Card>
  );
}
