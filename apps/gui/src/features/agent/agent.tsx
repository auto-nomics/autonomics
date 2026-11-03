/**
 * Shared agent helpers and the spawn dialog. The old AgentView host frame
 * (center slot + inspector panel + dialogs + palette) moved into the
 * workbench shell: the workspace docks in the auxiliary bar, the inspector
 * lives in inspector.tsx, the dialogs in dialogs.tsx, the palette in
 * app/workbench/command-palette.tsx.
 */
import {
  CircleDashed,
  CircleSlash,
  Hourglass,
  LoaderCircle,
  PenLine,
  RefreshCw,
  Shrink,
  Wrench,
  XCircle,
} from "lucide-react";
import { useEffect, useState } from "react";
import { gatewayRequest } from "@/gateway";
import { Button } from "@/components/ui/button";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { Input } from "@/components/ui/input";
import { cn } from "@/lib/utils";

export type DisplaySettings = {
  collapse_thinking: boolean;
  collapse_tool_calls: boolean;
  collapse_tool_results: boolean;
};

export function preview(text: string) {
  const line = text.split(/\r?\n/).find((item) => item.trim()) ?? "";
  const collapsed = line.split(/\s+/).filter(Boolean).join(" ");
  return collapsed.length <= 100 ? collapsed : `${collapsed.slice(0, 99)}…`;
}

export async function copyText(text: string) {
  try {
    await navigator.clipboard.writeText(text);
    return "Copied message";
  } catch (cause) {
    const area = document.createElement("textarea");
    area.value = text;
    area.style.position = "fixed";
    area.style.opacity = "0";
    document.body.append(area);
    area.select();
    const copied = document.execCommand("copy");
    area.remove();
    return copied ? "Copied message" : `Copy failed: ${String(cause)}`;
  }
}

export function leafName(path: string) {
  return path.split("/").filter(Boolean).pop() ?? "";
}

/** Per-status icon + tone for agent lifecycle states. Backend source of
 *  truth is `AgentLifecycleStatus` (crates/agentik-types/src/lifecycle.rs),
 *  emitted directly by the daemon — there is no separate coarse status
 *  anymore. Every variant must land in one of these arms — unknown values
 *  fall through to the idle marker. A cleanly finished turn lands on
 *  `idle` too: the chat transcript shows the response, the icon rests. */
export function agentStatusIconMeta(status?: string) {
  const value = (status ?? "idle").toLowerCase();
  if (value.includes("error") || value.includes("fail")) return { Icon: XCircle, className: "text-destructive" };
  if (value.includes("retry")) return { Icon: RefreshCw, className: "animate-spin text-amber-400" };
  if (value.includes("tool")) return { Icon: Wrench, className: "animate-pulse text-amber-400" };
  if (value.includes("stream")) return { Icon: PenLine, className: "animate-pulse text-sky-400" };
  if (value.includes("request") || value === "running")
    return { Icon: LoaderCircle, className: "animate-spin text-sky-400" };
  if (value.includes("compact")) return { Icon: Shrink, className: "animate-pulse text-violet-400" };
  if (value.includes("waiting")) return { Icon: Hourglass, className: "text-violet-400" };
  if (value.includes("cancel") || value.includes("abort"))
    return { Icon: CircleSlash, className: "text-muted-foreground/70" };
  return { Icon: CircleDashed, className: "text-muted-foreground/60" };
}

export function AgentStatusIcon({ status, className }: { status?: string; className?: string }) {
  const { Icon, className: tone } = agentStatusIconMeta(status);
  return <Icon aria-hidden className={cn("size-3 shrink-0 motion-reduce:animate-none", tone, className)} />;
}

/** Status → color tones for composer accents (the rotating ring around
 * the send button while a turn runs, the in-flight chip). Arm order
 * mirrors agentStatusIconMeta so icon color and ring color always agree. */
export function agentStatusTone(status?: string) {
  const value = (status ?? "idle").toLowerCase();
  if (value.includes("error") || value.includes("fail")) {
    return { border: "border-destructive", text: "text-destructive" };
  }
  if (value.includes("retry") || value.includes("tool")) {
    return { border: "border-amber-400", text: "text-amber-400" };
  }
  if (value.includes("stream") || value.includes("request") || value === "running") {
    return { border: "border-sky-400", text: "text-sky-400" };
  }
  if (value.includes("compact") || value.includes("waiting")) {
    return { border: "border-violet-400", text: "text-violet-400" };
  }
  if (value.includes("cancel") || value.includes("abort")) {
    return { border: "border-muted-foreground/60", text: "text-muted-foreground" };
  }
  if (value.includes("completed")) {
    return { border: "border-emerald-500/80", text: "text-emerald-500/80" };
  }
  return { border: "border-muted-foreground/50", text: "text-muted-foreground" };
}

export function statusLabel(status?: string) {
  return (status ?? "idle").toLowerCase();
}

/** Spawn form used by the workbench shell (palette, workspace tab bar and
 *  title-bar actions all open it). */
export function SpawnAgentDialog({
  open,
  onOpenChange,
  busy,
  onSpawnAgent,
}: {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  busy: string | null;
  onSpawnAgent: (input: { name: string; parentPath: string; profile: string }) => Promise<void>;
}) {
  const [spawnName, setSpawnName] = useState("");
  const [spawnParent, setSpawnParent] = useState("/root");
  const [spawnProfile, setSpawnProfile] = useState("researcher");
  const [profiles, setProfiles] = useState(["researcher", "developer", "browser", "curator"]);
  useEffect(() => {
    if (!open) return;
    const controller = new AbortController();
    void gatewayRequest("GET", "/profiles", undefined, { signal: controller.signal }).then((result: unknown) => {
      if (controller.signal.aborted || !Array.isArray(result)) return;
      const options = result.filter((item): item is string => typeof item === "string" && item.length > 0);
      if (!options.length) return;
      setProfiles(options);
      setSpawnProfile(current => options.includes(current) ? current : options[0]);
    }).catch(() => { /* Built-in kinds keep the picker usable offline. */ });
    return () => controller.abort();
  }, [open]);

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent>
        <DialogHeader>
          <DialogTitle>New agent</DialogTitle>
          <DialogDescription>Spawn a resident agent under an existing path.</DialogDescription>
        </DialogHeader>
        <form
          className="grid gap-3"
          onSubmit={async (event) => {
            event.preventDefault();
            await onSpawnAgent({ name: spawnName, parentPath: spawnParent, profile: spawnProfile });
            onOpenChange(false);
          }}
        >
          <Input value={spawnName} placeholder="agent name" onChange={(event) => setSpawnName(event.target.value)} required />
          <Input value={spawnParent} placeholder="/root" onChange={(event) => setSpawnParent(event.target.value)} required />
          <select aria-label="Agent profile" value={spawnProfile} onChange={(event) => setSpawnProfile(event.target.value)} required className="h-9 w-full rounded-md border bg-background px-3 text-sm outline-none focus-visible:ring-2 focus-visible:ring-ring">
            {profiles.map(profile => <option key={profile} value={profile}>{profile}</option>)}
          </select>
          <Button type="submit" disabled={busy === "spawn"}>Spawn</Button>
        </form>
      </DialogContent>
    </Dialog>
  );
}
