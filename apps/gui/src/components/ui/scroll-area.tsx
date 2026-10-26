import * as React from "react"
import { ScrollArea as ScrollAreaPrimitive } from "@base-ui/react/scroll-area"
import { cn } from "cn"

type ScrollAreaProps = ScrollAreaPrimitive.Root.Props & {
  contentClassName?: string
  viewportRef?: React.Ref<HTMLDivElement>
  viewportClassName?: string
}

function ScrollArea({
  children,
  className,
  contentClassName,
  viewportRef,
  viewportClassName,
  ...props
}: ScrollAreaProps) {
  return (
    <ScrollAreaPrimitive.Root
      data-slot="scroll-area"
      className={cn("relative overflow-hidden rounded-[inherit]", className)}
      {...props}
    >
      <ScrollAreaPrimitive.Viewport
        ref={viewportRef}
        className={cn("size-full overscroll-contain rounded-[inherit] outline-none", viewportClassName)}
      >
        <ScrollAreaPrimitive.Content className={contentClassName}>
          {children}
        </ScrollAreaPrimitive.Content>
      </ScrollAreaPrimitive.Viewport>
      <ScrollBar orientation="vertical" />
      <ScrollBar orientation="horizontal" />
      <ScrollAreaPrimitive.Corner />
    </ScrollAreaPrimitive.Root>
  )
}

function ScrollBar({
  className,
  orientation = "vertical",
  ...props
}: ScrollAreaPrimitive.Scrollbar.Props) {
  return (
    <ScrollAreaPrimitive.Scrollbar
      data-slot="scroll-area-scrollbar"
      orientation={orientation}
      className={cn(
        "flex touch-none select-none p-px transition-colors data-horizontal:h-1.5 data-vertical:w-1.5",
        className
      )}
      {...props}
    >
      <ScrollAreaPrimitive.Thumb
        className="h-full w-full rounded-full bg-foreground/25 transition-colors hover:bg-foreground/35 data-scrolling:bg-foreground/40"
      />
    </ScrollAreaPrimitive.Scrollbar>
  )
}

export { ScrollArea, ScrollBar }
