/**
 * 插件管理面 — the manager entry of the plugin development loop:
 * installed (runtime-active) plugins above, development projects below.
 * Creation binds an approved environment and scaffolds a runnable demo
 * node; opening a dev window connects to the project's container
 * (editor + terminal + validate/activate/test-run live there).
 */
import { useCallback, useEffect, useState } from "react";
import { FlaskConical, FolderOpen, Package, RefreshCw, RotateCcw, Trash2 } from "lucide-react";
import { gatewayRequest } from "@/gateway";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { openEditor } from "@/app/workbench/workbench-store";

type Project = { name: string; status: string | null; image: string | null; error: string | null };
type InstalledPlugin = {
  name: string;
  status: string;
  image: string | null;
  kinds: string[];
  registered: boolean;
};
type Environment = { id: string; reference: string };

export function openPluginDevWindow(name: string): void {
  openEditor({ kind: "plugin-dev", params: { name } });
}

export function ProjectEditor() {
  const [projects, setProjects] = useState<Project[]>([]);
  const [installed, setInstalled] = useState<InstalledPlugin[]>([]);
  const [environments, setEnvironments] = useState<Environment[]>([]);
  const [error, setError] = useState("");
  const [busy, setBusy] = useState(false);
  const [creating, setCreating] = useState(false);
  const [name, setName] = useState("");
  const [environment, setEnvironment] = useState("");
  const [purpose, setPurpose] = useState("");

  const refresh = useCallback(async () => {
    try {
      const [projectList, pluginList, env] = await Promise.all([
        gatewayRequest("GET", "/plugin-projects"),
        gatewayRequest("GET", "/plugins"),
        gatewayRequest("GET", "/plugins/environments"),
      ]);
      setProjects(projectList.projects ?? []);
      setInstalled(pluginList.plugins ?? []);
      setEnvironments(env.environments ?? []);
      setEnvironment((current) => current || env.environments?.[0]?.id || "");
      setError("");
    } catch (cause: unknown) {
      setError(String((cause as { message?: string } | null)?.message ?? cause));
    }
  }, []);
  useEffect(() => { void refresh(); }, [refresh]);

  const create = async (event: React.FormEvent) => {
    event.preventDefault();
    setBusy(true);
    setError("");
    try {
      await gatewayRequest("POST", "/plugin-projects", { name, environment_id: environment, purpose });
      setName("");
      setPurpose("");
      setCreating(false);
      await refresh();
      openPluginDevWindow(name);
    } catch (cause: unknown) {
      setError(String((cause as { message?: string } | null)?.message ?? cause));
    } finally {
      setBusy(false);
    }
  };

  const pluginAction = async (plugin: string, action: "uninstall" | "rollback") => {
    const message = action === "uninstall"
      ? `卸载插件 ${plugin}?(工作区与快照保留)`
      : `回滚 ${plugin} 到上一份快照?`;
    if (!window.confirm(message)) return;
    setBusy(true);
    try {
      await gatewayRequest("POST", `/plugins/${encodeURIComponent(plugin)}/${action}`);
      await refresh();
    } catch (cause: unknown) {
      setError(String((cause as { message?: string } | null)?.message ?? cause));
    } finally {
      setBusy(false);
    }
  };

  const devNames = new Set(projects.map((item) => item.name));

  return (
    <section aria-label="插件管理" className="flex min-h-0 min-w-0 flex-1 flex-col">
      <header className="flex flex-wrap items-center gap-2 border-b px-4 py-3">
        <Package className="size-4 text-primary" />
        <h1 className="mr-auto font-semibold">插件管理</h1>
        <Button size="sm" variant="outline" onClick={() => void refresh()}><RefreshCw /> 刷新</Button>
        <Button size="sm" onClick={() => setCreating(!creating)}>新建插件</Button>
      </header>
      {error && <div role="alert" className="border-b border-destructive/30 bg-destructive/10 px-4 py-2 text-sm text-destructive">{error}</div>}
      {creating && (
        <form onSubmit={(event) => void create(event)} className="grid gap-2 border-b bg-muted/30 p-4 sm:grid-cols-2">
          <label className="grid gap-1 text-xs">插件名称
            <Input required pattern="[a-z][a-z0-9-]*[a-z0-9]|[a-z]" value={name} onChange={(event) => setName(event.target.value)} placeholder="my-plugin" />
          </label>
          <label className="grid gap-1 text-xs">运行环境(已批准)
            <select required className="h-9 rounded-md border bg-background px-2 text-sm" value={environment} onChange={(event) => setEnvironment(event.target.value)}>
              <option value="">选择环境</option>
              {environments.map((item) => <option key={item.id} value={item.id}>{item.id}</option>)}
            </select>
          </label>
          <label className="grid gap-1 text-xs sm:col-span-2">用途
            <Input required value={purpose} onChange={(event) => setPurpose(event.target.value)} />
          </label>
          <div className="flex gap-2 sm:col-span-2">
            <Button disabled={busy || !environment} size="sm" type="submit">创建并打开开发窗口</Button>
            <Button size="sm" type="button" variant="ghost" onClick={() => setCreating(false)}>取消</Button>
          </div>
        </form>
      )}

      <div className="min-h-0 flex-1 overflow-y-auto">
        <div className="px-4 pt-3 text-xs font-semibold uppercase text-muted-foreground">已安装插件</div>
        {installed.length === 0 && <p className="px-4 py-2 text-xs text-muted-foreground">暂无已安装插件。开发项目「激活」后会出现在这里。</p>}
        {installed.map((plugin) => (
          <div key={plugin.name} data-installed-plugin={plugin.name} className="flex items-center gap-2 border-b px-4 py-2 text-sm">
            <div className="min-w-0 flex-1">
              <div className="flex items-center gap-2">
                <span className="truncate font-medium">{plugin.name}</span>
                {plugin.registered && <span className="rounded bg-emerald-500/15 px-1.5 py-0.5 text-[10px] text-emerald-600">已注册</span>}
                <span className="truncate text-xs text-muted-foreground">{plugin.status}</span>
              </div>
              <div className="truncate text-[10px] text-muted-foreground" title={plugin.image ?? ""}>{plugin.kinds.join(" · ")}</div>
            </div>
            {devNames.has(plugin.name) && (
              <Button size="sm" variant="ghost" title="打开开发窗口" onClick={() => openPluginDevWindow(plugin.name)}><FlaskConical /></Button>
            )}
            <Button size="sm" variant="ghost" aria-label={`回滚 ${plugin.name}`} title="回滚到上一份快照" disabled={busy} onClick={() => void pluginAction(plugin.name, "rollback")}><RotateCcw /></Button>
            <Button size="sm" variant="ghost" aria-label={`卸载 ${plugin.name}`} title="卸载(保留工作区)" disabled={busy} onClick={() => void pluginAction(plugin.name, "uninstall")}><Trash2 /></Button>
          </div>
        ))}

        <div className="px-4 pt-4 text-xs font-semibold uppercase text-muted-foreground">开发项目</div>
        {projects.length === 0 && <p className="px-4 py-2 text-xs text-muted-foreground">暂无开发项目。</p>}
        {projects.map((item) => (
          <div key={item.name} data-dev-project={item.name} className="flex items-center gap-2 border-b px-4 py-2 text-sm">
            <div className="min-w-0 flex-1">
              <span className="truncate font-medium">{item.name}</span>
              <span className="ml-2 truncate text-xs text-muted-foreground">{item.error ? "需要修复" : item.status}</span>
            </div>
            <Button size="sm" variant="outline" onClick={() => openPluginDevWindow(item.name)}>
              <FolderOpen /> 开发窗口
            </Button>
          </div>
        ))}
      </div>
    </section>
  );
}
