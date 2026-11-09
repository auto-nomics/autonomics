/**
 * HPC/计算目标域的前端类型,与网关 `/api/v1/compute`、`/api/v1/hpc/jobs`
 * 的 serde 形状一一对应。后端结构带 `deny_unknown_fields`,这里只声明
 * UI 消费的字段;未列出的字段按未知字段忽略处理会有风险,因此提交时
 * 只回传从后端拿到的完整对象或显式构造的子集。
 */

export type SlurmSettings = {
  enabled?: boolean;
  ssh_host?: string;
  ssh_command?: string;
  scratch_remote_dir?: string;
  artifact_root?: string | null;
  typed_worker_config?: string | null;
  apptainer_manifest?: string | null;
  partition?: string;
  gpu_partition?: string;
  gpu_cpus_per_gpu?: number;
  gpu_qos?: string | null;
  account?: string | null;
  qos?: string | null;
  mem_per_cpu?: string;
  default_time?: string;
  poll_interval_secs?: number;
  panel_cache_remote_dir?: string;
  tools_enabled?: boolean;
  data_ssh_host?: string | null;
  pi_ssh_host?: string | null;
  remote_python?: string;
  [key: string]: unknown;
};

export type ComputeBackend =
  | { type: "local" }
  | { type: "slurm"; settings: SlurmSettings }
  | { type: "ssh"; settings: SlurmSettings };

export type SshEndpoint = {
  hostname: string;
  user: string;
  port: number;
  host_key_alias: string;
};

export type ComputeTarget = {
  id: string;
  revision: number;
  name: string;
  enabled: boolean;
  backend: ComputeBackend;
  data_domains?: DataDomain[];
  capsule_sha256?: Record<string, string>;
  endpoints?: Record<string, SshEndpoint>;
};

export type DataDomain = {
  id: string;
  ssh_aliases: string[];
  remote_root: string;
  vfs_prefix: string;
  retained_vfs_prefix?: string | null;
  retained_remote_root?: string | null;
};

export type DataBackend =
  | { type: "local"; root: string }
  | { type: "ssh"; alias: string; root: string; python: string };

export type DataSource = {
  id: string;
  name: string;
  backend: DataBackend;
  read_only?: boolean;
  endpoint?: SshEndpoint | null;
  permissions?: unknown;
};

export type ComputePolicy = {
  revision: number;
  target_id: string;
  target_revision: number;
};

export type ComputeRegistryState = {
  deleted_target_ids?: string[];
  version: number;
  revision: number;
  policy: ComputePolicy;
  targets: ComputeTarget[];
  data_sources: DataSource[];
};

export type ExecutionBinding = {
  id: string;
  target_id: string;
  target_revision: number;
  policy_revision: number;
  target_sha256: string;
};

export type MountDefinition = {
  path: string;
  backend?: string;
  read_only?: boolean;
  [key: string]: unknown;
};

export type ComputeStatus = {
  deployment: Record<string, unknown> & { source_compatible?: boolean };
  state: ComputeRegistryState;
  mounts: MountDefinition[];
  runs: ExecutionBinding[];
};

export type ReadinessBlocker = {
  code: string;
  [key: string]: unknown;
};

export type ReadinessReport = {
  state: string;
  blockers?: ReadinessBlocker[];
  deployment?: Record<string, unknown>;
  [key: string]: unknown;
};

export type MonitorView = {
  target_id?: string;
  target_revision?: number;
  id: string;
  agent_id: string;
  agent_path: string;
  session_id: string;
  submission_key: string;
  cluster: string;
  job_id?: string | null;
  state: string;
  scheduler_state?: string | null;
  notification: string;
  last_error?: string | null;
  next_poll: number;
  updated_at: number;
  scientific_admission_granted: boolean;
};

export type HpcJobsResponse = {
  monitors: MonitorView[];
  scientific_admission_granted: boolean;
};
