import { Dialog, DialogContent, DialogTitle } from "@/components/ui/dialog";
import { CanvasAnnotations } from "./canvas-annotations";
import { useCanvasAgentHandoff, type CanvasAgentHandoffProps } from "./use-canvas-agent-handoff";
export function CanvasAnnotationDialog({ open, onOpenChange, agentPath, ...props }: CanvasAgentHandoffProps & { open: boolean; onOpenChange: (open: boolean) => void; agentPath: string }) {
  const handoff = useCanvasAgentHandoff({ ...props, target: agentPath });
  return <Dialog open={open} onOpenChange={onOpenChange}><DialogContent className="max-h-[80vh] overflow-y-auto"><DialogTitle>对象批注</DialogTitle>
    <CanvasAnnotations {...props} preparing={handoff.busy} canPrepare={handoff.targetAvailable} onPrepare={async annotation => {
      const prepared = await handoff.prepare(annotation.note, annotation);
      if (prepared) onOpenChange(false);
      return prepared;
    }} />
    {handoff.error && <p role="alert" className="text-destructive">{handoff.error}</p>}
  </DialogContent></Dialog>;
}
