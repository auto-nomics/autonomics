export type LifecycleStatus =
  | "Idle"
  | "Waiting"
  | "Requesting"
  | "Streaming"
  | "ExecutingTool"
  | "Compacting"
  | "Cancelled"
  | "Error";

export type PlanStep = {
  title: string;
  status: "Pending" | "InProgress" | "Completed" | "Cancelled";
};

export type GatewayFrame = {
  seq: number;
  kind: "agent" | "host" | "notice" | "lag";
  data: any;
};

export type ChatItem = {
  id: number;
  role: "user" | "assistant" | "thinking" | "tool" | "error" | "status";
  text: string;
  usage?: string;
  kind?: "thinking" | "tool_call" | "tool_result";
  time: number;
};

export type AgentInfo = {
  path: string;
  agent_id: string | null;
  status: string;
  model?: string | null;
  context_length?: number | null;
};

export type SessionInfo = {
  id: string;
  title?: string | null;
  created_at: number;
  last_active: number;
  telemetry?: {
    input_tokens?: number;
    output_tokens?: number;
    total_tokens?: number;
    total_tool_use?: number;
    time_consume_ms?: number;
  };
};

export type BackendMessage = {
  id: string;
  role: "user" | "assistant";
  content?: Array<{
    type?: string;
    text?: string;
    thinking?: string;
    name?: string;
    is_error?: boolean;
    content?: string | null;
  }>;
};

export type StoredAgent = {
  id: string;
  name: string;
  config_json: {
    kind?: string;
    preferred_model?: string;
    runtime?: AgentRuntimeOverrides;
  };
  created_at: number;
  last_active: number;
};

export type ModelRow = {
  model_name: string;
  provider_name: string;
  context_length: number;
  max_output_tokens: number;
  vision_ability: boolean;
  supports_function_calling: boolean;
  supports_streaming: boolean;
  supports_thinking: boolean;
  thinking_enabled: boolean;
  input_token_price: number;
  output_token_price: number;
  custom: boolean;
};

export type OpenaiTokenState = {
  present: boolean;
  expired: boolean;
};

export type CustomModelDraft = {
  provider_name: string;
  original_model_name: string | null;
  model_name: string;
  context_length: string;
  max_output_tokens: string;
  vision_ability: boolean;
  supports_function_calling: boolean;
  supports_streaming: boolean;
  supports_thinking: boolean;
  thinking_enabled: boolean;
  input_token_price: string;
  output_token_price: string;
};

export type ProviderRow = {
  id: number;
  name: string;
  provider_type: string;
  base_url: string;
  api_key: string;
  auth_method: string;
};

export type StateSnapshot = {
  last_seq: number;
  active_model_spec: string | null;
  profiles: any[];
  agents: AgentInfo[];
  sessions: Record<string, any[]>;
  display_settings: {
    collapse_thinking: boolean;
    collapse_tool_calls: boolean;
    collapse_tool_results: boolean;
  };
  model_catalog: {
    providers: ProviderRow[];
    models: ModelRow[];
    active_model: string | null;
    openai_token?: OpenaiTokenState | null;
  };
};

export type EmbeddingConfig = {
  enabled: boolean;
  base_url: string;
  model: string;
  batch_size: number;
  timeout_secs: number;
};

export type BibArticle = {
  id: string;
  title: string;
  authors?: Array<{ fore_name?: string | null; last_name?: string | null; name?: string | null }>;
  abstract_text?: string | null;
  year?: number | null;
  journal?: string | null;
  identifiers?: Array<{ kind: string; value: string }>;
  keywords?: string[];
  source?: string;
};

export type BibCollection = {
  parent_id?: string | null;
  collection_id?: number;
  library_id?: number;
  key?: string;
  deleted?: boolean;
  direct_article_count?: number;
  child_count?: number;
  id: string;
  name: string;
  description?: string | null;
  article_ids: string[];
  tags?: string[];
  status?: string;
};

export type BibAttachment = { key: string; link_mode: number; content_type?: string | null; path?: string | null; title?: string | null; source_url?: string | null; vfs_path?: string | null; original_filename?: string | null; file_size?: number | null; storage_hash?: string | null };

export type BibFullText = {
  attachment_key?: string | null;
  file_format?: string | null;
  file_size?: number | null;
  source?: string | null;
  text_content?: string | null;
  file_path?: string;
  extracted_by?: string | null;
  extract_status?: string | null;
};

export type BibAnnotation = {
  id: string;
  comment?: string;
  tags?: string[];
  kind?: string;
  content: string;
  page?: number | null;
  color?: string | null;
  rects?: Array<{ left: number; top: number; width: number; height: number }> | null;
};

export type BibRequest = {
  article_id: string;
  collection_id: string;
  position: number;
  role: string;
  fetch_status: string;
  added_by: string;
  note?: string | null;
};

export type BibDetail = {
  article: BibArticle;
  attachments?: BibAttachment[];
  fulltext: BibFullText | null;
  annotations: BibAnnotation[];
  fulltext_pagination?: {
    offset: number;
    limit: number;
    total_chars: number;
    truncated: boolean;
    next_offset?: number | null;
  };
};

export type KmsWorker = {
  worker: string;
  state: string;
  running: boolean;
  last_run_at: number | null;
  last_error: string | null;
  total_claimed: number;
  total_executed: number;
  total_released: number;
  at_ms: number;
};

export type KmsKnowledgeNode = {
  id: string;
  parent_id: string | null;
  path: string;
  title: string;
  body_md: string;
  status: string;
  confidence: number;
  source_hash: string;
  structural: boolean;
  created_at: number;
  updated_at: number;
};

export type KmsSkillArtifact = {
  relative_path: string;
  content: string;
};

export type KmsSkill = {
  id: string;
  skill_name: string;
  node_path: string;
  version: number;
  title: string;
  description: string;
  tags: string[];
  tier: string;
  source_tier: string;
  created_at: number;
  artifact_paths: string[];
};

export type KmsProposal = {
  id: string;
  skill_name: string;
  node_path: string;
  status: string;
  title: string;
  body_md: string;
  rationale: string;
  source_evidence_ids: string[];
  artifacts: KmsSkillArtifact[];
  content_hash: string;
  approved_skill_version_id: string | null;
  created_at: number;
  updated_at: number;
};

export type KmsEvidence = {
  id: string;
  source_kind: string;
  source_id: string;
  evidence_kind: string;
  summary: string;
  body: string;
  anchor_path: string | null;
  error: string | null;
  status: string;
  occurrence_count: number;
  created_at: number;
  last_seen_at: number;
};

export type SkillLibraryRow = {
  name: string;
  tier: string;
  tags: string[];
  description: string;
  installed: boolean;
  proposal_status: string | null;
  proposal_update: boolean;
  usage_gets: number;
  usage_search_hits: number;
  usage_runs: number;
  usage_evals: number;
  usage_last_used: number;
};

export type SkillLibraryDetail = SkillLibraryRow & {
  body: string;
  workflows: string[];
  evals: string[];
  evidence_count: number;
};

export type AgentRuntimeOverrides = {
  use_memory?: boolean | null;
  generate_memory?: boolean | null;
  reasoning_effort?:
    | "none"
    | "minimal"
    | "low"
    | "medium"
    | "high"
    | "xhigh"
    | "max"
    | "ultra"
    | null;
};

export type AgentRuntimeConfigView = {
  runtime: AgentRuntimeOverrides;
  effective: {
    use_memory: boolean;
    generate_memory: boolean;
    reasoning_effort?: string | null;
  };
};

export type DagPort = {
  index: number;
  label?: string | null;
  data_type: string;
};

export type DagNode = {
  id: string;
  kind: string;
  status: unknown;
  dirty: boolean;
  inputs: DagPort[];
  outputs: DagPort[];
};

export type DagEdge = {
  from: string;
  from_port: number;
  to: string;
  to_port: number;
};

export type DagSnapshot = {
  nodes: DagNode[];
  edges: DagEdge[];
};

export type KmsRow = {
  key: string;
  name: string;
  tier: string;
  description: string;
  tags: string[];
  body: string;
  version?: number;
  installed: boolean;
  proposal_status: string | null;
  proposal_update: boolean;
  proposal: KmsProposal | null;
};
