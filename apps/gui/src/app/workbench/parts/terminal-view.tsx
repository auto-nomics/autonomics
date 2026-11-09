import { useEffect, useState } from "react";
import { ProjectTerminal } from "@/features/projects/project-terminal";
import { useActiveEditorContext } from "../active-editor-context";

/** Terminal sessions belong to the workbench, independently of editor tabs. */
export function TerminalView({ visible }: { visible: boolean }) {
  const { editor } = useActiveEditorContext();
  const project = editor?.kind === "plugin-projects" ? (editor.state as { project?: string; containerVersion?: number } | null) : null;
  const [sessions, setSessions] = useState<Record<string, number>>({});
  const [selected, setSelected] = useState("");
  useEffect(() => {
    if (!visible || !project?.project) return;
    const name = project.project, version = project.containerVersion ?? 0;
    setSessions(current => current[name] === version ? current : { ...current, [name]: version });
    setSelected(current => current || name);
  }, [visible, project?.project, project?.containerVersion]);
  return <div hidden={!visible} style={{ display: visible ? undefined : "none" }} className="flex min-h-0 flex-1 flex-col">
    {Object.keys(sessions).length > 0 ? <label className="border-b px-2 py-1 text-xs">终端项目 <select aria-label="终端项目" className="ml-2 bg-background" value={selected} onChange={event => setSelected(event.target.value)}>{Object.keys(sessions).map(name => <option key={name} value={name}>{name}</option>)}</select></label> : <div className="p-4 text-xs text-muted-foreground">请选择插件项目。</div>}
    {Object.entries(sessions).map(([name, version]) => <div key={`${name}:${version}`} hidden={selected !== name} style={{ display: selected === name ? undefined : "none" }} className="flex min-h-0 flex-1 flex-col"><ProjectTerminal project={name} /></div>)}
  </div>;
}
