import { useSyncExternalStore } from "react";
import { enrollComputeTarget, type EnrollComputeRequest, type EnrollmentEvent } from "./api";

export type EnrollmentRun = {
  targetId: string;
  startedAt: number;
  endedAt?: number;
  status: "running" | "success" | "error" | "unknown";
  events: (EnrollmentEvent & { at: number })[];
  stage: string;
  steps: string[];
  uploaded?: number;
  total?: number;
  error?: string;
};

// A navigation change does not abandon the request or hide its final outcome.
let current: EnrollmentRun | null = null;
const listeners = new Set<() => void>();
function publish(run: EnrollmentRun | null) {
  current = run;
  listeners.forEach(listener => listener());
}
export function useEnrollmentRun() {
  return useSyncExternalStore(
    listener => { listeners.add(listener); return () => { listeners.delete(listener); }; },
    () => current,
    () => null,
  );
}
export async function startEnrollment(request: EnrollComputeRequest) {
  if (current?.status === "running") throw new Error("已有目标正在安装，请等待完成。");
  publish({ targetId: request.id, startedAt: Date.now(), status: "running", stage: "validate", events: [], steps: [
    "validate", "authenticate",
    ...(request.deploy_worker ? ["prepare_directory", "check_python", "upload", "commit_upload", "chmod"] : []),
    "probe_worker", "worker_profile", ...(request.data_alias ? ["data_source"] : []),
    "save_target", ...(request.select ? ["select_target"] : []),
  ] });
  try {
    const state = await enrollComputeTarget(request, event => {
      if (!current) return;
      publish({
        ...current, stage: event.stage,
        uploaded: event.uploaded_bytes ?? current.uploaded,
        total: event.total_bytes ?? current.total,
        events: [...current.events.slice(-499), { ...event, at: Date.now() }],
      });
    });
    publish({ ...current!, status: "success", endedAt: Date.now() });
    window.dispatchEvent(new Event("autonomics:compute-changed"));
    return state;
  } catch (error) {
    const message = error instanceof Error ? error.message : String(error);
    const uncertain = message.includes("安装进度连接已断开") || error instanceof TypeError;
    publish({ ...current!, status: uncertain ? "unknown" : "error", endedAt: Date.now(), error: message });
    window.dispatchEvent(new Event("autonomics:compute-changed"));
    throw error;
  }
}
export function clearEnrollment() {
  if (current?.status !== "running") publish(null);
}
