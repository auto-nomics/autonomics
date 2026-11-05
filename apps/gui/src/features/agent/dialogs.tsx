/**
 * Agent dialogs: resume persisted agent, copy message, prompt history.
 * Relocated from the retired AgentView so they exist independently of any
 * view; mounted once by the workbench shell. Registers their commands
 * (owner "agent-dialogs") and the Ctrl+R prompt-history shortcut.
 */

import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { gatewayRequest } from "@/gateway";
import type { AgentInfo, ChatItem, StoredAgent } from "@/types";
import { Button } from "@/components/ui/button";
import { Card } from "@/components/ui/card";
import { Dialog, DialogContent, DialogDescription, DialogHeader, DialogTitle } from "@/components/ui/dialog";
import { Input } from "@/components/ui/input";
import { ScrollArea } from "@/components/ui/scroll-area";
import { registerCommands } from "@/app/workbench/command-registry";
import { cn } from "@/lib/utils";
import { copyText, leafName, preview } from "./agent";

type PickerItem = { id: string; role: "user" | "assistant"; text: string };

/** Imperative open hooks: the shell's inspector buttons and commands can
 *  open a dialog without prop drilling through the pane. */
export type AgentDialogKind = "resume" | "copy" | "history";
const dialogOpenListeners = new Set<(kind: AgentDialogKind) => void>();
export function openAgentDialog(kind: AgentDialogKind): void {
  for (const listener of dialogOpenListeners) listener(kind);
}

export type AgentDialogsProps = {
  activeAgent: string;
  agents: AgentInfo[];
  messages: ChatItem[];
  promptHistory: string[];
  /** Restores a prompt into the workspace composer. */
  onDraftChange: (value: string) => void;
  onResumeAgent: (agent: StoredAgent) => Promise<void>;
  /** Admin shutdown used by the delete flow (delete-agent only). */
  onShutdownAgent: (path: string) => Promise<void>;
  /** Closes the tab of a deleted dormant identity (view-only op). */
  onCloseTab?: (path: string) => void;
};

export function AgentDialogs(props: AgentDialogsProps) {
  const [resumeOpen, setResumeOpen] = useState(false);
  const [resumeQuery, setResumeQuery] = useState("");
  const [storedAgents, setStoredAgents] = useState<StoredAgent[]>([]);
  const [resumeBusy, setResumeBusy] = useState<string | null>(null);
  const [renameAgent, setRenameAgent] = useState<{ id: string; name: string } | null>(null);
  const [deleteAgent, setDeleteAgent] = useState<{ id: string; confirm: string } | null>(null);
  const [picker, setPicker] = useState<{ open: boolean; query: string }>({ open: false, query: "" });
  const [historyOpen, setHistoryOpen] = useState(false);
  const [historyQuery, setHistoryQuery] = useState("");
  const [toast, setToast] = useState<string | null>(null);

  const showToast = useCallback((message: string) => {
    setToast(message);
    window.setTimeout(() => setToast((current) => current === message ? null : current), 2200);
  }, []);

  // Bounded server-side search for the resume dialog — one page per
  // (debounced) query, never the full identities table.
  const resumeSearchTimer = useRef<ReturnType<typeof setTimeout> | null>(null);
  const loadStoredAgents = useCallback(async (query = "") => {
    try {
      const suffix = query.trim()
        ? `?q=${encodeURIComponent(query.trim())}&limit=100`
        : "?limit=100";
      setStoredAgents(await gatewayRequest("GET", `/storage/agents${suffix}`));
    } catch (cause) {
      showToast(`Failed to load persisted agents: ${String(cause)}`);
    }
  }, [showToast]);

  const searchStoredAgentsDebounced = (query: string) => {
    if (resumeSearchTimer.current) clearTimeout(resumeSearchTimer.current);
    resumeSearchTimer.current = setTimeout(() => void loadStoredAgents(query), 250);
  };

  const openResume = useCallback(() => {
    setResumeQuery("");
    setResumeOpen(true);
    void loadStoredAgents();
  }, [loadStoredAgents]);

  const openCopy = useCallback(() => {
    if (!props.messages.some((message) => (message.role === "user" || message.role === "assistant") && message.text.trim())) {
      showToast("No text messages available");
      return;
    }
    setPicker({ open: true, query: "" });
  }, [props.messages, showToast]);

  const openHistory = useCallback(() => setHistoryOpen(true), []);

  const selectPickerItem = async (item: PickerItem) => {
    setPicker((current) => ({ ...current, open: false }));
    showToast(await copyText(item.text));
  };

  // Shell wiring: the inspector Copy/History buttons and palette commands
  // route through the imperative hooks above.
  useEffect(() => {
    const listener = (kind: AgentDialogKind) => {
      if (kind === "resume") openResume();
      else if (kind === "copy") openCopy();
      else setHistoryOpen(true);
    };
    dialogOpenListeners.add(listener);
    return () => {
      dialogOpenListeners.delete(listener);
    };
  }, [openResume, openCopy]);

  // Commands behind the palette; revoked on unmount or when handlers change.
  useEffect(() => registerCommands("agent-dialogs", [
    { id: "resume-agent", title: "Resume agent", category: "agent", keywords: "restore reconnect continue previous", run: openResume },
    { id: "history", title: "Search prompt history", category: "agent", keywords: "ctrl r recall previous prompt", run: openHistory },
    { id: "copy", title: "Copy message", category: "clipboard", keywords: "clipboard transcript text", run: openCopy },
  ]), [openResume, openHistory, openCopy]);

  useEffect(() => {
    const onKeyDown = (event: KeyboardEvent) => {
      if ((event.ctrlKey || event.metaKey) && event.key.toLowerCase() === "r" && props.activeAgent) {
        event.preventDefault();
        setHistoryOpen(true);
      }
    };
    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
  }, [props.activeAgent]);

  const pickerItems = useMemo(() => {
    return [...props.messages]
      .filter((message) => (message.role === "user" || message.role === "assistant") && message.text.trim())
      .reverse()
      .map((message) => ({ id: String(message.id), role: message.role as "user" | "assistant", text: message.text }));
  }, [props.messages]);

  const filteredPickerItems = useMemo(() => {
    const needle = picker.query.trim().toLowerCase();
    return pickerItems.filter((item) => !needle || `${item.role} ${item.text}`.toLowerCase().includes(needle));
  }, [picker.query, pickerItems]);

  const filteredStored = useMemo(() => {
    const needle = resumeQuery.trim().toLowerCase();
    return storedAgents
      .filter((agent) => !needle || `${agent.name} ${agent.config_json.kind ?? ""}`.toLowerCase().includes(needle))
      .sort((left, right) => right.last_active - left.last_active);
  }, [resumeQuery, storedAgents]);

  const filteredHistory = useMemo(() => {
    const needle = historyQuery.trim().toLowerCase();
    return [...props.promptHistory]
      .filter((item): item is string => typeof item === "string")
      .reverse()
      .filter((item) => !needle || item.toLowerCase().includes(needle));
  }, [historyQuery, props.promptHistory]);

  return (
    <>
      <Dialog open={resumeOpen} onOpenChange={setResumeOpen}>
        <DialogContent className="max-w-3xl">
          <DialogHeader>
            <DialogTitle>Resume persisted agent</DialogTitle>
            <DialogDescription>Restore a stored agent. Existing runtime instances are focused.</DialogDescription>
          </DialogHeader>
          <Input
            value={resumeQuery}
            placeholder="Search by path or profile"
            onChange={(event) => {
              setResumeQuery(event.target.value);
              searchStoredAgentsDebounced(event.target.value);
            }}
          />
          <ScrollArea className="max-h-[60vh] min-h-0" contentClassName="space-y-2">
            {filteredStored.slice(0, 300).map((agent) => {
              const running = props.agents.find((item) => item.agent_id === agent.id);
              return (
                <Card key={agent.id} className="p-2">
                  <div className="flex flex-wrap items-start justify-between gap-2">
                    <div className="min-w-0">
                      <div className="truncate text-sm font-medium">{agent.name}</div>
                      <div className="truncate text-xs text-muted-foreground">
                        {agent.config_json.kind ?? "researcher"} · {agent.config_json.preferred_model ?? "default model"} · {new Date(agent.last_active).toLocaleString()}
                      </div>
                    </div>
                    <div className="flex flex-wrap gap-1">
                      <Button
                        size="xs"
                        disabled={resumeBusy === agent.id}
                        onClick={async () => {
                          setResumeBusy(agent.id);
                          await props.onResumeAgent(agent);
                          setResumeBusy(null);
                          setResumeOpen(false);
                        }}
                      >
                        {running ? "Focus" : resumeBusy === agent.id ? "Resuming…" : "Resume"}
                      </Button>
                      <Button
                        size="xs"
                        variant="secondary"
                        onClick={() => setRenameAgent(renameAgent?.id === agent.id ? null : { id: agent.id, name: leafName(agent.name) })}
                      >
                        Rename
                      </Button>
                      <Button size="xs" variant="destructive" onClick={() => setDeleteAgent({ id: agent.id, confirm: "" })}>Delete</Button>
                    </div>
                  </div>
                  {renameAgent?.id === agent.id && (
                    <form
                      className="mt-2 flex gap-1.5"
                      onSubmit={async (event) => {
                        event.preventDefault();
                        const segments = agent.name.split("/").filter(Boolean);
                        const nextPath = segments.length <= 1 ? `/root/${renameAgent.name}` : `/${[...segments.slice(0, -1), renameAgent.name].join("/")}`;
                        await gatewayRequest("PATCH", `/storage/agents/${encodeURIComponent(agent.id)}`, { path: nextPath });
                        setRenameAgent(null);
                        await loadStoredAgents();
                      }}
                    >
                      <Input value={renameAgent.name} onChange={(event) => setRenameAgent({ id: agent.id, name: event.target.value })} />
                      <Button type="submit" size="sm">Save</Button>
                    </form>
                  )}
                  {deleteAgent?.id === agent.id && (
                    <form
                      className="mt-2 flex gap-1.5"
                      onSubmit={async (event) => {
                        event.preventDefault();
                        if (deleteAgent.confirm.trim().toLowerCase() !== "yes") return;
                        await gatewayRequest("DELETE", `/storage/agents/${encodeURIComponent(agent.id)}`);
                        const runningAgent = props.agents.find((item) => item.agent_id === agent.id);
                        if (runningAgent) await props.onShutdownAgent(runningAgent.path);
                        // A deleted identity has no view to keep — close
                        // its (possibly dormant) tab if one is open.
                        else props.onCloseTab?.(agent.name);
                        setDeleteAgent(null);
                        await loadStoredAgents();
                      }}
                    >
                      <Input value={deleteAgent.confirm} placeholder='type "yes"' onChange={(event) => setDeleteAgent({ id: agent.id, confirm: event.target.value })} />
                      <Button type="submit" size="sm" variant="destructive">Delete forever</Button>
                    </form>
                  )}
                </Card>
              );
            })}
          </ScrollArea>
        </DialogContent>
      </Dialog>

      <Dialog open={picker.open} onOpenChange={(open) => setPicker((current) => ({ ...current, open }))}>
        <DialogContent className="max-w-2xl">
          <DialogHeader>
            <DialogTitle>Copy message</DialogTitle>
            <DialogDescription>Click a transcript message to copy it.</DialogDescription>
          </DialogHeader>
          <Input value={picker.query} placeholder="Filter messages" onChange={(event) => setPicker((current) => ({ ...current, query: event.target.value }))} />
          <ScrollArea className="max-h-[60vh] min-h-0" contentClassName="space-y-1">
            {filteredPickerItems.map((item, index) => (
              <button
                key={item.id}
                type="button"
                onClick={() => void selectPickerItem(item)}
                className={cn("grid w-full grid-cols-[44px_minmax(0,1fr)] gap-2 rounded-md border p-2 text-left hover:bg-accent", index === 0 && "border-primary bg-accent")}
              >
                <span className="text-xs text-muted-foreground">{item.role === "user" ? "YOU" : "AI"}</span>
                <span className="truncate text-sm">{preview(item.text)}</span>
              </button>
            ))}
            {!filteredPickerItems.length && <div className="p-4 text-sm text-muted-foreground">No matching messages</div>}
          </ScrollArea>
        </DialogContent>
      </Dialog>

      <Dialog open={historyOpen} onOpenChange={setHistoryOpen}>
        <DialogContent>
          <DialogHeader>
            <DialogTitle>Prompt history</DialogTitle>
            <DialogDescription>Select a previous prompt to restore it in the composer.</DialogDescription>
          </DialogHeader>
          <Input value={historyQuery} placeholder="Filter prompts" onChange={(event) => setHistoryQuery(event.target.value)} />
          <ScrollArea className="max-h-[60vh] min-h-0" contentClassName="space-y-1">
            {filteredHistory.map((item, index) => (
              <button
                key={`${index}-${item}`}
                type="button"
                onClick={() => {
                  props.onDraftChange(item);
                  setHistoryOpen(false);
                }}
                className="w-full truncate rounded-md border p-2 text-left text-sm hover:bg-accent"
              >
                {preview(item)}
              </button>
            ))}
            {!filteredHistory.length && <div className="p-4 text-sm text-muted-foreground">No matching prompts</div>}
          </ScrollArea>
        </DialogContent>
      </Dialog>

      {toast && (
        <div className="fixed bottom-5 left-1/2 z-[120] -translate-x-1/2 rounded-md border border-primary/40 bg-popover px-3 py-2 text-sm shadow-lg">
          {toast}
        </div>
      )}
    </>
  );
}
