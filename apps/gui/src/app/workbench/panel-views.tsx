import i18n from "@/i18n";
import { useTranslation } from "react-i18next";
import { Activity, Bell, Terminal } from "lucide-react";
import { ScrollArea } from "@/components/ui/scroll-area";
import { Badge } from "@/components/ui/badge";
import { useWorkbenchRuntime } from "./runtime-context";
import { registerWorkbenchView } from "./view-registry";
import { TerminalView } from "./parts/terminal-view";

function EventsView() {
  const { t } = useTranslation("workbench");
  const runtime = useWorkbenchRuntime();
  return <ScrollArea className="min-h-0 flex-1" contentClassName="p-2 font-mono text-[11px]">
    {runtime.frameLog.map((entry, index) => <div key={`${entry.seq}-${index}`} className="flex gap-2 py-px"><span className="shrink-0 text-muted-foreground/70">{new Date(entry.time).toLocaleTimeString()}</span><Badge variant="outline">{entry.kind}</Badge><span className="min-w-0 truncate">{entry.summary}</span></div>)}
    {!runtime.frameLog.length && <div className="p-4 text-xs text-muted-foreground">{t("panel.eventsEmpty")}</div>}
  </ScrollArea>;
}
function NoticesView() {
  const { t } = useTranslation("workbench");
  const runtime = useWorkbenchRuntime();
  return <ScrollArea className="min-h-0 flex-1" contentClassName="p-2 text-xs">
    {runtime.notices.map((entry, index) => <div key={`${entry.seq}-${index}`} className="flex gap-2 rounded-md border p-1.5"><span className="shrink-0 text-muted-foreground">{new Date(entry.time).toLocaleTimeString()}</span><span className="min-w-0 break-all">{JSON.stringify(entry.data)}</span></div>)}
    {!runtime.notices.length && <div className="p-4 text-muted-foreground">{t("panel.noticesEmpty")}</div>}
  </ScrollArea>;
}

function registerPanelViewTitles(): void {
registerWorkbenchView({ id: "events", title: i18n.t("workbench:views.events"), icon: Activity, defaultLocation: "panel", body: EventsView });
registerWorkbenchView({ id: "notices", title: i18n.t("workbench:views.notices"), icon: Bell, defaultLocation: "panel", body: NoticesView });
registerWorkbenchView({ id: "terminal", title: i18n.t("workbench:views.terminal"), icon: Terminal, defaultLocation: "panel", body: () => <TerminalView visible /> });
}

registerPanelViewTitles();
// Re-register on language change so view titles follow the active locale.
i18n.on("languageChanged", registerPanelViewTitles);
