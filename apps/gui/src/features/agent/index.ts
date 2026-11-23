import i18next from "i18next";
import zhAgent from "@/i18n/locales/zh/agent.json";
import enAgent from "@/i18n/locales/en/agent.json";

// Register the "agent" namespace catalogues at module init so components can
// useTranslation("agent") without touching the central i18n bootstrap.
if (!i18next.hasResourceBundle("zh", "agent")) i18next.addResourceBundle("zh", "agent", zhAgent);
if (!i18next.hasResourceBundle("en", "agent")) i18next.addResourceBundle("en", "agent", enAgent);

export {
  AgentStatusIcon,
  SpawnAgentDialog,
  agentStatusIconMeta,
  agentStatusTone,
  copyText,
  leafName,
  preview,
  statusLabel,
  type DisplaySettings,
} from "./agent";
export { AgentWorkspace, type AgentWorkspaceMode } from "./agent-workspace";
export { AgentComposer, EFFORT_OPTIONS, effortLabel, modelShortLabel } from "./composer";
export { AgentInspector } from "./inspector";
export { AgentDialogs, openAgentDialog, type AgentDialogKind } from "./dialogs";
export { layoutSideOpen, readAgentLayout, writeAgentLayout } from "./agent-layout";
