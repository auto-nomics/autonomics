import { useWikiDocument } from "./wiki-document";
/**
 * Wiki editing shell: title, metadata in the scrolling document body,
 * Vditor, and a dirty-guarded exit back to reading mode.
 */
import { Loader2 } from "lucide-react";
import { useCallback, useState, type ReactNode } from "react";
import { toast } from "sonner";

import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { newId } from "@/lib/utils";
import { VditorEditor } from "./vditor-editor";
import { ModeSwitch, type WikiMode } from "./reading-view";
import { formatTime, putEntryContent, updateEntryMeta, type WikiEntry } from "./wiki-api";

export function EditorView({
  entry: initialEntry,
  navigation,
  onModeChange,
  onSaved,
}: {
  entry: WikiEntry;
  navigation?: ReactNode;
  onModeChange: (mode: WikiMode) => void;
  onSaved: (entry: WikiEntry, stayEditing: boolean) => void;
}) {
  const { title, alias, tags, content, saving, pendingImages, entry, model } = useWikiDocument(initialEntry);
  const setTitle = (title: string) => model.update({ title });
  const setAlias = (alias: string) => model.update({ alias });
  const setTags = (tags: string) => model.update({ tags });
  const setContent = (content: string) => model.update({ content });
  const setSaving = (saving: boolean) => model.update({ saving });
  const [uploadSource] = useState(newId);
  const setUploading = useCallback((pending: boolean) => model.setUploading(uploadSource, pending), [model, uploadSource]);
  const uploading = pendingImages > 0;

  const split = (value: string) =>
    value.split(",").map((item) => item.trim()).filter(Boolean);

  const dirty =
    title !== entry.title ||
    alias !== entry.alias.join(", ") ||
    tags !== entry.tags.join(", ") ||
    content !== entry.content;

  async function save() {
    if (model.state.saving || model.state.pendingImages > 0) return;
    const { title, alias, tags, content, entry } = model.state;
    const trimmed = title.trim();
    if (!trimmed) {
      toast.error("标题不能为空");
      return;
    }
    setSaving(true);
    try {
      const aliasList = split(alias);
      const tagList = split(tags);
      let latest = entry;
      if (
        trimmed !== entry.title ||
        aliasList.join(",") !== entry.alias.join(",") ||
        tagList.join(",") !== entry.tags.join(",")
      ) {
        latest = await updateEntryMeta(entry.id, {
          new_title: trimmed !== entry.title ? trimmed : undefined,
          alias: aliasList,
          tags: tagList,
        });
      }
      if (content !== entry.content) {
        await putEntryContent(entry.id, content);
      }
      const saved = { ...latest, title: trimmed, alias: aliasList, tags: tagList, content };
      model.accept(saved, { title, alias, tags, content });
      onSaved(saved, model.dirty || model.state.pendingImages > 0);
      toast.success("已保存(新版本快照已追加)");
    } catch (error) {
      toast.error((error as Error).message);
    } finally {
      setSaving(false);
    }
  }

  const requestDone = () => {
    if (dirty && !window.confirm("有未保存的修改,确定返回阅读模式?")) return;
    if (dirty) model.update({ title: entry.title, alias: entry.alias.join(", "), tags: entry.tags.join(", "), content: entry.content });
    onModeChange("read");
  };

  return (
    <div className="flex min-h-0 flex-1 flex-col overflow-hidden" onKeyDown={event => {
      if ((event.ctrlKey || event.metaKey) && event.key.toLowerCase() === "s") {
        event.preventDefault(); event.stopPropagation();
        if (!saving) void save();
      }
    }}>
      <div className="shrink-0 border-b px-2 py-1.5">
        {/* min-w-0: 行是网格/flex 布局的子项,长标题 input 的固有宽度
            会把保存按钮挤出视口(移动端实测)。 */}
        <div className="flex min-w-0 flex-wrap items-center gap-2">
          {navigation}
          <input
            id="wiki-title"
            value={title}
            placeholder="标题(唯一,自动 trim)"
            spellCheck={false}
            className="min-w-0 flex-1 bg-transparent text-[15px] leading-6 font-semibold outline-none placeholder:text-sm placeholder:font-normal placeholder:text-muted-foreground"
            onChange={(event) => setTitle(event.target.value)}
          />
          {dirty ? (
            <Badge variant="outline" className="shrink-0 border-amber-500/40 text-[10px] text-amber-400">
              未保存
            </Badge>
          ) : null}
          <ModeSwitch
            mode="edit"
            onChange={(mode) => {
              if (mode === "read") requestDone();
            }}
          />
          <Button
            size="sm"
            onClick={() => void save()}
            disabled={saving || uploading || !title.trim()}
            title="保存 Ctrl+S"
          >
            {saving ? <Loader2 className="h-4 w-4 animate-spin" /> : null} 保存
          </Button>
        </div>
      </div>
      <VditorEditor
        key={entry.id}
        entryId={entry.id}
        initialContent={content}
        getContent={() => model.state.content}
        onChange={setContent}
        onUploadsChange={setUploading}
        header={
          /* Obsidian-style properties card at the top of the note body,
             mirroring the reading view; alias/tags are editable here. */
          <div className="wiki-properties wiki-properties--edit mx-auto mb-4 max-w-3xl text-xs">
            <div className="wiki-properties-row">
              <span className="wiki-properties-key">aliases</span>
              <Input
                id="wiki-alias"
                className="h-6 border-0 bg-transparent px-1 text-[13px] shadow-none focus-visible:ring-0"
                placeholder="别名(逗号分隔)"
                value={alias}
                onChange={(event) => setAlias(event.target.value)}
              />
            </div>
            <div className="wiki-properties-row">
              <span className="wiki-properties-key">tags</span>
              <Input
                id="wiki-tags"
                className="h-6 border-0 bg-transparent px-1 text-[13px] shadow-none focus-visible:ring-0"
                placeholder="标签(逗号分隔)"
                value={tags}
                onChange={(event) => setTags(event.target.value)}
              />
            </div>
            <div className="wiki-properties-row">
              <span className="wiki-properties-key">created</span>
              <span className="text-muted-foreground">{formatTime(entry.ctime)}</span>
            </div>
            <div className="wiki-properties-row">
              <span className="wiki-properties-key">modified</span>
              <span className="text-muted-foreground">{formatTime(entry.mtime)}</span>
            </div>
          </div>
        }
      />
    </div>
  );
}
