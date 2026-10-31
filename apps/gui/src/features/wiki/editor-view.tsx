/**
 * SiYuan-style 编辑模式 shell: an inline document title (SiYuan edits the
 * doc title in place), a collapsible attributes row (alias / tags) and the
 * block editor — plus a dirty-guarded exit back to 阅读模式.
 */
import { ChevronDown, Loader2 } from "lucide-react";
import { useEffect, useState } from "react";
import { toast } from "sonner";

import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Card, CardContent, CardHeader } from "@/components/ui/card";
import { Input } from "@/components/ui/input";
import { BlockEditor } from "./block-editor";
import { ModeSwitch, type WikiMode } from "./reading-view";
import { putEntryContent, updateEntryMeta, type WikiEntry } from "./wiki-api";

export function EditorView({
  entry,
  onModeChange,
  onSaved,
}: {
  entry: WikiEntry;
  onModeChange: (mode: WikiMode) => void;
  onSaved: (entry: WikiEntry) => void;
}) {
  const [title, setTitle] = useState(entry.title);
  const [alias, setAlias] = useState(entry.alias.join(", "));
  const [tags, setTags] = useState(entry.tags.join(", "));
  const [content, setContent] = useState(entry.content);
  const [attrsOpen, setAttrsOpen] = useState(false);
  const [saving, setSaving] = useState(false);

  const split = (value: string) =>
    value.split(",").map((item) => item.trim()).filter(Boolean);

  const dirty =
    title !== entry.title ||
    alias !== entry.alias.join(", ") ||
    tags !== entry.tags.join(", ") ||
    content !== entry.content;

  async function save() {
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
      onSaved({ ...latest, title: trimmed, alias: aliasList, tags: tagList, content });
      toast.success("已保存(新版本快照已追加)");
    } catch (error) {
      toast.error((error as Error).message);
    } finally {
      setSaving(false);
    }
  }

  // Ctrl/Cmd+S anywhere in the editor saves (also swallows the browser's
  // page-save default while the user is editing).
  useEffect(() => {
    const onKey = (event: KeyboardEvent) => {
      if ((event.ctrlKey || event.metaKey) && event.key.toLowerCase() === "s") {
        event.preventDefault();
        if (!saving) void save();
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  });

  const requestDone = () => {
    if (dirty && !window.confirm("有未保存的修改,确定返回阅读模式?")) return;
    onModeChange("read");
  };

  return (
    <Card className="flex h-full min-h-0 flex-col">
      <CardHeader className="gap-2 pb-2">
        <div className="flex items-center gap-2">
          <input
            id="wiki-title"
            value={title}
            placeholder="标题(唯一,自动 trim)"
            spellCheck={false}
            className="min-w-0 flex-1 bg-transparent text-lg font-semibold outline-none placeholder:text-sm placeholder:font-normal placeholder:text-muted-foreground"
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
        <div>
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
                  className="h-8"
                  value={alias}
                  onChange={(event) => setAlias(event.target.value)}
                />
              </div>
              <div className="grid gap-1">
                <span className="text-xs text-muted-foreground">标签(逗号分隔)</span>
                <Input
                  id="wiki-tags"
                  className="h-8"
                  value={tags}
                  onChange={(event) => setTags(event.target.value)}
                />
              </div>
            </div>
          ) : null}
        </div>
      </CardHeader>
      <CardContent className="flex min-h-0 flex-1 flex-col">
        <BlockEditor
          key={entry.id}
          entryId={entry.id}
          initialContent={content}
          onChange={setContent}
        />
      </CardContent>
    </Card>
  );
}
