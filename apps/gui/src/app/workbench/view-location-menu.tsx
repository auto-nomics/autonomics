import { Ellipsis, GripVertical, PanelBottom, PanelLeft, PanelRight } from "lucide-react";
import { useState } from "react";
import { Button } from "@/components/ui/button";
import { DropdownMenu, DropdownMenuContent, DropdownMenuGroup, DropdownMenuLabel, DropdownMenuRadioGroup, DropdownMenuRadioItem, DropdownMenuTrigger } from "@/components/ui/dropdown-menu";
import { startViewDrag } from "./view-drag";
import { moveWorkbenchView, useViewLayout, workbenchViewLocation } from "./view-layout";
import { workbenchView, type ViewLocation } from "./view-registry";
import { setPanelVisible } from "./workbench-store";

const locations = [
  { value: "primarySidebar", label: "主侧边栏", icon: PanelLeft },
  { value: "secondarySidebar", label: "辅助侧边栏", icon: PanelRight },
  { value: "panel", label: "底部面板", icon: PanelBottom },
] as const;

export function ViewLocationMenu({ id, primaryOnly = false }: { id: string; primaryOnly?: boolean }) {
  const layout = useViewLayout();
  const [open, setOpen] = useState(false);
  const location = workbenchViewLocation(id, layout);
  if (primaryOnly && location !== "primarySidebar") return null;
  return <span className="flex shrink-0 items-center">
    <span draggable onDragStart={event => { setOpen(false); startViewDrag(event, id); }} aria-label="拖动视图" title="拖动以移动视图" className="grid size-5 cursor-grab place-items-center text-muted-foreground/50 hover:text-foreground active:cursor-grabbing"><GripVertical className="size-3" /></span>
    <DropdownMenu open={open} onOpenChange={setOpen}>
      <DropdownMenuTrigger render={<Button size="icon-xs" variant="ghost" aria-label="视图位置" title="移动视图（也可拖动）" />}><Ellipsis /></DropdownMenuTrigger>
      <DropdownMenuContent align="end" className="min-w-44">
        <DropdownMenuGroup>
          <DropdownMenuLabel>移动{workbenchView(id)?.title ?? "视图"}到</DropdownMenuLabel>
          <DropdownMenuRadioGroup value={location} onValueChange={value => {
            setOpen(false);
            moveWorkbenchView(id, value as ViewLocation);
            if (value === "panel") setPanelVisible(true);
          }}>
            {locations.map(item => <DropdownMenuRadioItem key={item.value} value={item.value}><item.icon />{item.label}</DropdownMenuRadioItem>)}
          </DropdownMenuRadioGroup>
        </DropdownMenuGroup>
      </DropdownMenuContent>
    </DropdownMenu>
  </span>;
}
