/**
 * HPC/计算目标 API 封装。所有调用走统一网关请求(会话 Cookie 鉴权,
 * token 不进前端)。
 */
import { gatewayRequest } from "@/gateway";
import type {
  ComputeCatalog,
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
): Promise<ComputeCatalog> {
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
  },
): Promise<ComputeCatalog> {
  return gatewayRequest("PUT", "/compute/targets", {
    expected_revision: expectedRevision,
    ...target,
  });
}

export async function addComputeDataSource(
  expectedRevision: number,
  source: Record<string, unknown>,
): Promise<ComputeCatalog> {
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
