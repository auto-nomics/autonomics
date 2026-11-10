import { useEffect, useState } from "react";
import { Eye, FileText, Link2, MousePointer2, Unlink, X } from "lucide-react";
import { Button } from "@/components/ui/button";
import { Command, CommandEmpty, CommandGroup, CommandInput, CommandItem, CommandList } from "@/components/ui/command";
import { Dialog, DialogContent, DialogDescription, DialogHeader, DialogTitle } from "@/components/ui/dialog";
import type { ContextResource } from "@/features/context/context-model";
import { editorKind } from "./editor-registry";
import { allGroups, getWorkbenchState, useWorkbenchState } from "./workbench-store";

/** The host owns one binding per agent, including before its first session.
 * Resolve the document again on send: document models may change independently
 * of the workbench layout and must never be captured in a stale render. */
export function useAgentContextBinding() {
  const workbench = useWorkbenchState();
  const [mode, setMode] = useState("");
  const [pickerOpen, setPickerOpen] = useState(false);
  const [previewOpen, setPreviewOpen] = useState(false);
  const candidates = allGroups(workbench.groups).flatMap(([, group]) => group.editors)
    .filter(editor => Boolean(editorKind(editor.kind)?.agentContext));

  useEffect(() => {
    if (mode && mode !== "$follow" && !candidates.some(editor => editor.id === mode)) setMode("");
  }, [mode, workbench.groups]);

  const getResource = (): ContextResource | null => {
    const current = getWorkbenchState();
    const id = mode === "$follow" ? current.lastContextEditorId : mode;
    const editor = allGroups(current.groups).flatMap(([, group]) => group.editors).find(editor => editor.id === id);
    const kind = editor && editorKind(editor.kind);
    const snapshot = editor && kind?.agentContext?.(editor);
    if (!editor || !kind || !snapshot) return null;
    return {
      kind: "editor", id: editor.id, title: kind.title(editor), source: editor.id,
      scope: typeof snapshot.scope === "string" ? snapshot.scope : "文档当前状态（每次发送时更新）",
      revision: String(current.revision), content: JSON.stringify(snapshot, null, 2),
    };
  };
  const resource = getResource();
  const label = mode === "$follow" ? `跟随 · ${resource?.title ?? "待选择文档"}` : resource?.title ?? "绑定文档";
  const choose = (value: string) => { setMode(value); setPickerOpen(false); };

  const control = <div className="flex min-w-0 items-center gap-0.5" data-agent-context-binding="">
    <Button size="xs" variant={mode ? "secondary" : "ghost"} className="min-w-0 shrink justify-start"
      aria-label="绑定文档上下文" title={mode ? `${label} · 点击更换` : "选择文档，每次发送时附加最新状态"}
      onClick={() => setPickerOpen(true)}>
      {mode === "$follow" ? <MousePointer2 data-icon="inline-start" /> : <Link2 data-icon="inline-start" />}
      <span className="max-w-48 truncate">{label}</span>
    </Button>
    {resource && <Button size="icon-xs" variant="ghost" aria-label="预览文档上下文" title="预览下次发送时附加的内容" onClick={() => setPreviewOpen(true)}><Eye /></Button>}
    {mode && <Button size="icon-xs" variant="ghost" aria-label="解除文档绑定" title="解除文档绑定" onClick={() => setMode("")}><X /></Button>}
    <Dialog open={pickerOpen} onOpenChange={setPickerOpen}>
      <DialogContent className="sm:max-w-md">
        <DialogHeader>
          <DialogTitle>绑定文档上下文</DialogTitle>
          <DialogDescription>每次发送消息时附加所选文档的最新状态。可预览内容，或随时解除绑定。</DialogDescription>
        </DialogHeader>
        <Command>
          <CommandInput placeholder="搜索已打开的文档…" />
          <CommandList>
            <CommandEmpty>没有匹配的文档，请先打开文档再绑定。</CommandEmpty>
            <CommandGroup heading="绑定方式">
              <CommandItem value="不绑定文档" data-checked={!mode} onSelect={() => choose("")}><Unlink /><span>不绑定文档</span></CommandItem>
              <CommandItem value="跟随当前文档" data-checked={mode === "$follow"} onSelect={() => choose("$follow")}>
                <MousePointer2 /><div className="flex min-w-0 flex-1 flex-col gap-0.5"><span>跟随当前文档</span><span className="truncate text-xs text-muted-foreground">{resource && mode === "$follow" ? resource.title : "使用最近查看的文档，切回对话后继续保留"}</span></div>
              </CommandItem>
            </CommandGroup>
            <CommandGroup heading="固定文档">
              {candidates.map(editor => {
                const kind = editorKind(editor.kind)!;
                const Icon = kind.icon?.(editor) ?? FileText;
                return <CommandItem key={editor.id} value={`${kind.title(editor)} ${editor.id}`} data-checked={mode === editor.id} onSelect={() => choose(editor.id)}>
                  <Icon /><span className="min-w-0 flex-1 truncate">{kind.title(editor)}</span>
                </CommandItem>;
              })}
              {!candidates.length && <p className="px-2 py-3 text-xs text-muted-foreground">打开知识词条、阅读器、幻灯片或插件项目后，即可在这里选择。</p>}
            </CommandGroup>
          </CommandList>
        </Command>
      </DialogContent>
    </Dialog>
    <Dialog open={previewOpen && Boolean(resource)} onOpenChange={setPreviewOpen}>
      <DialogContent className="max-h-[80dvh] overflow-y-auto sm:max-w-2xl">
        <DialogHeader><DialogTitle>{resource?.title}</DialogTitle><DialogDescription>{resource?.scope}</DialogDescription></DialogHeader>
        <pre className="whitespace-pre-wrap break-words text-xs">{resource?.content}</pre>
      </DialogContent>
    </Dialog>
  </div>;
  return { control, getResource };
}
