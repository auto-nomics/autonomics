import { useTranslation } from "react-i18next";
import { Ellipsis, GripVertical, PanelBottom, PanelLeft, PanelRight } from "lucide-react";
import { useState } from "react";
import { Button } from "@/components/ui/button";
import { DropdownMenu, DropdownMenuContent, DropdownMenuGroup, DropdownMenuLabel, DropdownMenuRadioGroup, DropdownMenuRadioItem, DropdownMenuTrigger } from "@/components/ui/dropdown-menu";
import { startViewDrag } from "./view-drag";
import { moveWorkbenchView, useViewLayout, workbenchViewLocation } from "./view-layout";
import { workbenchView, type ViewLocation } from "./view-registry";
import { setPanelVisible } from "./workbench-store";

const locations = [
  { value: "primarySidebar", key: "viewLocation.primarySidebar", icon: PanelLeft },
  { value: "secondarySidebar", key: "viewLocation.secondarySidebar", icon: PanelRight },
  { value: "panel", key: "viewLocation.panel", icon: PanelBottom },
] as const;

export function ViewLocationMenu({ id, primaryOnly = false }: { id: string; primaryOnly?: boolean }) {
  const { t } = useTranslation("workbench");
  const layout = useViewLayout();
  const [open, setOpen] = useState(false);
  const location = workbenchViewLocation(id, layout);
  if (primaryOnly && location !== "primarySidebar") return null;
  return <span className="flex shrink-0 items-center">
    <span draggable onDragStart={event => { setOpen(false); startViewDrag(event, id); }} aria-label={t("viewLocation.dragAria")} title={t("viewLocation.dragTitle")} className="grid size-5 cursor-grab place-items-center text-muted-foreground/50 hover:text-foreground active:cursor-grabbing"><GripVertical className="size-3" /></span>
    <DropdownMenu open={open} onOpenChange={setOpen}>
      <DropdownMenuTrigger render={<Button size="icon-xs" variant="ghost" aria-label={t("viewLocation.menuAria")} title={t("viewLocation.menuTitle")} />}><Ellipsis /></DropdownMenuTrigger>
      <DropdownMenuContent align="end" className="min-w-44">
        <DropdownMenuGroup>
          <DropdownMenuLabel>{t("viewLocation.moveTo", { name: workbenchView(id)?.title ?? t("views.view") })}</DropdownMenuLabel>
          <DropdownMenuRadioGroup value={location} onValueChange={value => {
            setOpen(false);
            moveWorkbenchView(id, value as ViewLocation);
            if (value === "panel") setPanelVisible(true);
          }}>
            {locations.map(item => <DropdownMenuRadioItem key={item.value} value={item.value}><item.icon />{t(item.key)}</DropdownMenuRadioItem>)}
          </DropdownMenuRadioGroup>
        </DropdownMenuGroup>
      </DropdownMenuContent>
    </DropdownMenu>
  </span>;
}
