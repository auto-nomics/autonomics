/**
 * Runtime context: the single useAgentRuntime() instance lives in App; the
 * workbench shell provides it here so editor bodies (config/dag and later
 * feature editors) can read agent state without prop drilling.
 */

import { createContext, useContext } from "react";
import type { useAgentRuntime } from "@/app/use-agent-runtime";

export type AgentRuntime = ReturnType<typeof useAgentRuntime>;

export const WorkbenchRuntimeContext = createContext<AgentRuntime | null>(null);

export function useWorkbenchRuntime(): AgentRuntime {
  const runtime = useContext(WorkbenchRuntimeContext);
  if (!runtime) throw new Error("useWorkbenchRuntime used outside WorkbenchShell");
  return runtime;
}
