/**
 * HPC/计算目标 API 封装。所有调用走统一网关请求(会话 Cookie 鉴权,
 * token 不进前端)。
 */
import { gatewayEnrollment, gatewayRequest } from "@/gateway";
import type {
  ComputeBackend,
  ComputeRegistryState,
  DataDomain,
  ComputeStatus,
  HpcJobsResponse,
  ComputeTargetCheck,
} from "./types";

export async function fetchComputeStatus(): Promise<ComputeStatus> {
  return gatewayRequest("GET", "/compute");
}

export async function selectComputeTarget(
  targetId: string,
): Promise<ComputeRegistryState> {
  return gatewayRequest("PUT", "/compute/policy", {
    target_id: targetId,
  });
}

export async function deleteComputeTarget(targetId: string): Promise<ComputeRegistryState> {
  return gatewayRequest("DELETE", `/compute/targets/${encodeURIComponent(targetId)}`);
}

export type TargetMountSettings = {
  alias: string;
  root: string;
  python: string;
};

export async function saveComputeTarget(
  target: {
    id: string;
    name: string;
    enabled: boolean;
    backend: ComputeBackend;
    data_domains?: DataDomain[];
  },
  mount?: TargetMountSettings,
): Promise<ComputeRegistryState> {
  if (mount) {
    return gatewayRequest("PUT", "/compute/target-data-source", { target, ...mount });
  }
  return gatewayRequest("PUT", "/compute/targets", target);
}

export type EnrollComputeRequest = {
  backend?: "ssh" | "slurm";
  container_runtime:
    | { type: "sif"; image_manifest: string; runtime: "apptainer" | "singularity_ce" }
    | { type: "podman"; plugins_root: string; plugins_sha256: string; podman: {
      executable: string; executable_sha256: string; image_catalog: string; image_catalog_sha256: string;
    } };
  id: string;
  name?: string;
  login_alias: string;
  worker_executable?: string;
  data_alias?: string;
  data_root?: string;
  storage_root?: string;
  partition?: string;
  account?: string;
  scratch_dir?: string;
  select: boolean;
  deploy_worker?: string;
  python?: string;
  ssh_command?: string;
};

export type EnrollmentEvent = { stage: string; message: string; uploaded_bytes?: number; total_bytes?: number };

export async function enrollComputeTarget(
  request: EnrollComputeRequest,
  onProgress: (event: EnrollmentEvent) => void,
): Promise<ComputeRegistryState> {
  return gatewayEnrollment(request, onProgress);
}

export async function checkComputeTarget(id: string): Promise<ComputeTargetCheck> {
  return gatewayRequest("POST", `/compute/targets/${encodeURIComponent(id)}/check`);
}

export async function fetchHpcJobs(): Promise<HpcJobsResponse> {
  return gatewayRequest("GET", "/hpc/jobs");
}
