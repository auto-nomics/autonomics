/**
 * Auxiliary agent pane: the chat slot (the single AgentWorkspace portals
 * here) plus the inspector as a resizable second column. The
 * workspace/inspector panel ids and the `autonomics.agent.layout`
 * persistence carry over unchanged from the old AgentView.
 */

import { useCallback, useEffect, useRef, useState } from "react";
import type { Layout, LayoutChangedMeta, PanelImperativeHandle } from "react-resizable-panels";
import { layoutSideOpen, readAgentLayout, writeAgentLayout, AgentInspector } from "@/features/agent";
import type { DisplaySettings } from "@/features/agent";
import type { AgentInfo } from "@/types";
import { ResizableHandle, ResizablePanel, ResizablePanelGroup } from "@/components/ui/resizable";

export type AuxAgentPaneProps = {
  /** Registers the chat slot DOM node (AgentWorkspace docked target). */
  onSlotChange: (slot: HTMLDivElement | null) => void;
  inspectorOpen: boolean;
  onSetInspectorOpen: (open: boolean) => void;
  activeAgent: string;
  activeInfo: AgentInfo | undefined;
  status?: string;
  plan: { revision: number; update: any } | null;
  usage: { input: number; output: number; cache: number } | null;
  display: DisplaySettings;
  onToggleDisplay: (key: keyof DisplaySettings) => void;
  onOpenDag: () => void;
  onOpenCopy: () => void;
  onOpenHistory: () => void;
  onRefreshHistory: () => void;
};

export function AuxAgentPane(props: AuxAgentPaneProps) {
  // Desktop layout: the inspector's width persists to localStorage; the
  // open flag lives in the workbench store so the workspace tab-bar toggle
  // and this panel stay in agreement.
  const rightPanelRef = useRef<PanelImperativeHandle | null>(null);
  const [defaultLayout] = useState<Layout | null>(() => {
    const saved = readAgentLayout();
    if (!saved) return null;
    // Prune retired panel ids (the agents sidebar was removed long ago):
    // the group only honors a defaultLayout whose key count equals the
    // mounted panel count.
    const panelIds = ["workspace", "inspector"];
    return Object.fromEntries(
      Object.entries(saved).filter(([id]) => panelIds.includes(id)),
    ) as Layout;
  });

  const handleLayoutChanged = useCallback((layout: Layout, meta: LayoutChangedMeta) => {
    writeAgentLayout(meta.requestedLayout ?? layout);
    props.onSetInspectorOpen(layoutSideOpen(layout, "inspector"));
  }, [props]);

  // Mirror the shared open flag onto the collapsible panel.
  useEffect(() => {
    const panel = rightPanelRef.current;
    if (!panel) return;
    if (props.inspectorOpen) {
      if (panel.isCollapsed()) panel.expand();
    } else if (!panel.isCollapsed()) {
      panel.collapse();
    }
  }, [props.inspectorOpen]);

  return (
    <ResizablePanelGroup
      orientation="horizontal"
      defaultLayout={defaultLayout ?? undefined}
      onLayoutChanged={handleLayoutChanged}
      className="min-h-0 min-w-0 flex-1"
    >
      {/* The chat column's floor is low: the aux pane is far narrower than
          the old full-width agent view, and a 30% floor would pin the
          inspector against its 280px default within ~3px of drag room. */}
      <ResizablePanel id="workspace" minSize="15%" className="min-w-0">
        {/* The workspace (tab bar, transcript, composer) is portaled into
            this slot from the shell so the same component instance survives
            layout and placement changes. */}
        <div ref={props.onSlotChange} className="flex h-full min-h-0 min-w-0 flex-col" />
      </ResizablePanel>
      <ResizableHandle aria-label="Resize inspector sidebar" />
      <ResizablePanel
        id="inspector"
        panelRef={rightPanelRef}
        defaultSize="280px"
        minSize="200px"
        maxSize="420px"
        collapsible
        collapsedSize="0%"
        className="bg-muted/50"
      >
        <aside className="flex h-full min-h-0 overflow-hidden border-l">
          <AgentInspector
            activeAgent={props.activeAgent}
            activeInfo={props.activeInfo}
            status={props.status}
            plan={props.plan}
            usage={props.usage}
            display={props.display}
            inspectorOpen={props.inspectorOpen}
            onToggleDisplay={props.onToggleDisplay}
            onOpenDag={props.onOpenDag}
            onOpenCopy={props.onOpenCopy}
            onOpenHistory={props.onOpenHistory}
            onRefreshHistory={props.onRefreshHistory}
          />
        </aside>
      </ResizablePanel>
    </ResizablePanelGroup>
  );
}
