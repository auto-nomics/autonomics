import { FolderOpen, RotateCcw, Trash2 } from "lucide-react";
import { Button } from "@/components/ui/button";
import { useTranslation } from "react-i18next";

export type PluginProject = { name: string; status: string | null; image: string | null; error: string | null };
export type InstalledPlugin = {
  name: string;
  status: string;
  image: string | null;
  kinds: string[];
  registered: boolean;
};

/** One row per plugin, combining its workspace and installed snapshot. */
export function PluginOverview({ projects, installed, busy, onOpen, onAction }: {
  projects: PluginProject[];
  installed: InstalledPlugin[];
  busy: boolean;
  onOpen: (name: string) => void;
  onAction: (name: string, action: "uninstall" | "rollback") => void;
}) {
  const { t } = useTranslation("projects");
  const names = [...new Set([...projects.map(item => item.name), ...installed.map(item => item.name)])];
  return <section aria-label={t("pluginManager.ariaLabel")} className="min-h-0 flex-1 overflow-y-auto p-4">
    <p className="mb-4 text-xs text-muted-foreground">{t("pluginManager.description")}</p>
    {names.length === 0 && <p className="py-8 text-center text-sm text-muted-foreground">{t("pluginManager.empty")}</p>}
    {names.map(name => {
      const project = projects.find(item => item.name === name);
      const plugin = installed.find(item => item.name === name);
      return <div key={name} data-plugin={name} data-installed-plugin={plugin ? name : undefined} data-dev-project={project ? name : undefined} className="flex flex-wrap items-center gap-2 border-b py-3 text-sm">
        <div className="min-w-0 flex-1 basis-48">
          <div className="flex items-center gap-2"><span className="truncate font-medium">{name}</span>{plugin?.registered && <span className="rounded bg-emerald-500/15 px-1.5 py-0.5 text-[10px] text-emerald-600">{t("pluginManager.registered")}</span>}</div>
          <div className="text-xs text-muted-foreground">{project && <span>{t("pluginManager.projectLabel")}{project.error ? t("pluginManager.needsRepair") : project.status ?? t("pluginManager.developing")} · </span>}{plugin ? t("pluginManager.installed", { status: plugin.status }) : t("pluginManager.notInstalled")}</div>
          {project?.error && <p className="text-xs text-destructive">{project.error}</p>}
          {(plugin?.image ?? project?.image) && <div className="truncate text-[10px] text-muted-foreground" title={plugin?.image ?? project?.image ?? ""}>{plugin?.image ?? project?.image}</div>}
          {!!plugin?.kinds.length && <div className="text-xs text-muted-foreground">{plugin.kinds.join(" · ")}</div>}
        </div>
        {project && <Button size="sm" variant="outline" disabled={busy} onClick={() => onOpen(name)}><FolderOpen /> {t("pluginManager.openProject")}</Button>}
        {plugin && <>
          <Button size="sm" variant="ghost" aria-label={t("pluginManager.rollbackLabel", { name })} title={t("pluginManager.rollbackTitle")} disabled={busy} onClick={() => onAction(name, "rollback")}><RotateCcw /></Button>
          <Button size="sm" variant="ghost" aria-label={t("pluginManager.uninstallLabel", { name })} title={t("pluginManager.uninstallTitle")} disabled={busy} onClick={() => onAction(name, "uninstall")}><Trash2 /></Button>
        </>}
      </div>;
    })}
  </section>;
}
