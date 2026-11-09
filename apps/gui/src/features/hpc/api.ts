/**
 * HPC/计算目标 API 封装。所有调用走统一网关请求(会话 Cookie 鉴权,
 * token 不进前端)。
 */
import { gatewayEnrollment, gatewayRequest } from "@/gateway";
import type {
  ComputeRegistryState,
  DataDomain,
  ComputeStatus,
  HpcJobsResponse,
  ReadinessReport,
} from "./types";

export async function fetchComputeStatus(): Promise<ComputeStatus> {
  return gatewayRequest("GET", "/compute");
}

export async function selectComputeTarget(
  expectedRevision: number,
  targetId: string,
): Promise<ComputeRegistryState> {
  return gatewayRequest("PUT", "/compute/policy", {
    expected_revision: expectedRevision,
    target_id: targetId,
  });
}

export async function saveComputeTarget(
  expectedRevision: number,
  target: {
    id: string;
    name: string;
    enabled: boolean;
    backend: Record<string, unknown>;
    data_domains?: DataDomain[];
  },
): Promise<ComputeRegistryState> {
  return gatewayRequest("PUT", "/compute/targets", {
    expected_revision: expectedRevision,
    ...target,
  });
}

export type EnrollComputeRequest = {
  expected_revision: number;
  id: string;
  name?: string;
  login_alias: string;
  worker_executable: string;
  data_alias?: string;
  data_root?: string;
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

export async function addComputeDataSource(
  expectedRevision: number,
  source: Record<string, unknown>,
): Promise<ComputeRegistryState> {
  return gatewayRequest("POST", "/compute/data-sources", {
    expected_revision: expectedRevision,
    source,
  });
}

export async function checkComputeTarget(id: string): Promise<ReadinessReport> {
  return gatewayRequest("POST", `/compute/targets/${encodeURIComponent(id)}/check`);
}

export async function fetchComputeReadiness(): Promise<ReadinessReport> {
  return gatewayRequest("POST", "/compute/readiness", {});
}

export async function fetchHpcJobs(): Promise<HpcJobsResponse> {
  return gatewayRequest("GET", "/hpc/jobs");
}
