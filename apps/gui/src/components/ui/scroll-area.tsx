import * as React from "react"
import { ScrollArea as ScrollAreaPrimitive } from "@base-ui/react/scroll-area"
import { cn } from "cn"

type ScrollAreaProps = ScrollAreaPrimitive.Root.Props & {
  contentClassName?: string
  scrollOrientation?: "vertical" | "both"
  viewportRef?: React.Ref<HTMLDivElement>
  viewportLabel?: string
  viewportTabIndex?: number
  viewportClassName?: string
}

function ScrollArea({
  children,
  className,
  contentClassName,
  scrollOrientation = "vertical",
  viewportRef,
  viewportClassName,
  viewportLabel,
  viewportTabIndex,
  ...props
}: ScrollAreaProps) {
  const supportsHorizontal = scrollOrientation === "both"

  return (
    <ScrollAreaPrimitive.Root
      data-slot="scroll-area"
      className={cn("relative overflow-hidden rounded-[inherit]", className)}
      {...props}
    >
      <ScrollAreaPrimitive.Viewport
        ref={viewportRef}
        data-slot="scroll-area-viewport"
        aria-label={viewportLabel}
        tabIndex={viewportTabIndex}
        className={cn("size-full overscroll-contain rounded-[inherit] outline-none", viewportClassName)}
      >
        <ScrollAreaPrimitive.Content
          data-slot="scroll-area-content"
          className={cn(!supportsHorizontal && "min-w-0!", contentClassName)}
        >
          {children}
        </ScrollAreaPrimitive.Content>
      </ScrollAreaPrimitive.Viewport>
      <ScrollBar orientation="vertical" />
      {supportsHorizontal && <ScrollBar orientation="horizontal" />}
      {supportsHorizontal && <ScrollAreaPrimitive.Corner />}
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
