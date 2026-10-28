import { Group, Panel, Separator, type GroupProps, type PanelProps, type SeparatorProps } from "react-resizable-panels"
import { cn } from "cn"

function ResizablePanelGroup({ className, ...props }: GroupProps) {
  return (
    <Group
      data-slot="resizable-panel-group"
      className={cn("h-full w-full", className)}
      {...props}
    />
  )
}

function ResizablePanel({ className, ...props }: PanelProps) {
  return (
    <Panel
      data-slot="resizable-panel"
      className={cn("h-full min-h-0", className)}
      {...props}
    />
  )
}

function ResizableHandle({
  withHandle,
  className,
  ...props
}: SeparatorProps & {
  withHandle?: boolean
}) {
  return (
    <Separator
      data-slot="resizable-handle"
      className={cn(
        "relative w-px shrink-0 bg-border outline-none transition-colors",
        "hover:bg-foreground/30 data-[separator=focus]:bg-ring focus-visible:ring-2 focus-visible:ring-ring/60",
        // Enlarge the grab area beyond the 1px line without moving it.
        "after:absolute after:-inset-y-2 after:-left-1 after:w-2.5",
        className
      )}
      {...props}
    >
      {withHandle && (
        <div className="pointer-events-none absolute top-1/2 left-1/2 flex h-8 w-3.5 -translate-x-1/2 -translate-y-1/2 items-center justify-center rounded-sm border bg-border">
          <div className="h-2.5 w-px rounded-full bg-muted-foreground/60" />
        </div>
      )}
    </Separator>
  )
}

export { ResizableHandle, ResizablePanel, ResizablePanelGroup }
