import { useWikiDocument } from "./wiki-document";
/**
 * SiYuan-style 编辑模式 shell: an inline document title (SiYuan edits the
 * doc title in place), a collapsible attributes row (alias / tags) and the
 * block editor — plus a dirty-guarded exit back to 阅读模式.
 */
import { ChevronDown, Loader2 } from "lucide-react";
import { useState, type ReactNode } from "react";
import { toast } from "sonner";

import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { BlockEditor } from "./block-editor";
import { ModeSwitch, type WikiMode } from "./reading-view";
import { putEntryContent, updateEntryMeta, type WikiEntry } from "./wiki-api";

export function EditorView({
  entry: initialEntry,
  navigation,
  onModeChange,
  onSaved,
}: {
  entry: WikiEntry;
  navigation?: ReactNode;
  onModeChange: (mode: WikiMode) => void;
  onSaved: (entry: WikiEntry) => void;
}) {
  const { title, alias, tags, content, saving, entry, model } = useWikiDocument(initialEntry);
  const setTitle = (title: string) => model.update({ title });
  const setAlias = (alias: string) => model.update({ alias });
  const setTags = (tags: string) => model.update({ tags });
  const setContent = (content: string) => model.update({ content });
  const setSaving = (saving: boolean) => model.update({ saving });
  const [attrsOpen, setAttrsOpen] = useState(false);

  const split = (value: string) =>
    value.split(",").map((item) => item.trim()).filter(Boolean);

  const dirty =
    title !== entry.title ||
    alias !== entry.alias.join(", ") ||
    tags !== entry.tags.join(", ") ||
    content !== entry.content;

  async function save() {
    if (model.state.saving) return;
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
      onSaved(saved);
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
            disabled={saving || !title.trim()}
            title="保存 Ctrl+S"
          >
            {saving ? <Loader2 className="h-4 w-4 animate-spin" /> : null} 保存
          </Button>
        </div>
        <div className="mt-1">
          <button
            type="button"
            className="flex items-center gap-1 text-xs text-muted-foreground transition-colors hover:text-foreground"
            onClick={() => setAttrsOpen((open) => !open)}
          >
            <ChevronDown className={`h-3.5 w-3.5 transition-transform ${attrsOpen ? "" : "-rotate-90"}`} />
            属性(别名 / 标签)
          </button>
          {attrsOpen ? (
            <div className="mt-2 grid gap-2 sm:grid-cols-2">
              <div className="grid gap-1">
                <span className="text-xs text-muted-foreground">别名(逗号分隔)</span>
                <Input
                  id="wiki-alias"
                  className="h-7 text-[13px]"
                  value={alias}
                  onChange={(event) => setAlias(event.target.value)}
                />
              </div>
              <div className="grid gap-1">
                <span className="text-xs text-muted-foreground">标签(逗号分隔)</span>
                <Input
                  id="wiki-tags"
                  className="h-7 text-[13px]"
                  value={tags}
                  onChange={(event) => setTags(event.target.value)}
                />
              </div>
            </div>
          ) : null}
        </div>
      </div>
      <BlockEditor
        key={entry.id}
        entryId={entry.id}
        initialContent={content}
        onChange={setContent}
      />
    </div>
  );
}
