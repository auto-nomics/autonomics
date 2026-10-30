import { useState } from "react";

import { Button } from "@/components/ui/button";
import {
  Card,
  CardContent,
  CardHeader,
  CardTitle,
} from "@/components/ui/card";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { putEntryContent, updateEntryMeta, type WikiEntry } from "@/lib/wiki-api";

interface EntryEditorProps {
  entry: WikiEntry;
  onCancel: () => void;
  onSaved: (entry: WikiEntry) => void;
}

/**
 * Metadata form + whole-body textarea. Saves in two calls (meta, then
 * content) — the server rebuilds search index, links, and a version
 * snapshot on each write.
 */
export function EntryEditor({ entry, onCancel, onSaved }: EntryEditorProps) {
  const [title, setTitle] = useState(entry.title);
  const [alias, setAlias] = useState(entry.alias.join(", "));
  const [tags, setTags] = useState(entry.tags.join(", "));
  const [content, setContent] = useState(entry.content);
  const [saving, setSaving] = useState(false);

  async function save() {
    setSaving(true);
    try {
      const aliasList = alias
        .split(",")
        .map((item) => item.trim())
        .filter(Boolean);
      const tagList = tags
        .split(",")
        .map((item) => item.trim())
        .filter(Boolean);
      const metaChanged =
        title !== entry.title || aliasList.join(",") !== entry.alias.join(",") || tagList.join(",") !== entry.tags.join(",");
      // Rename first so the write-back below addresses a stable id.
      if (metaChanged) {
        const updated = await updateEntryMeta(entry.id, {
          new_title: title !== entry.title ? title : undefined,
          alias: aliasList,
          tags: tagList,
        });
        onSaved(updated);
      }
      if (content !== entry.content) {
        await putEntryContent(entry.id, content);
      }
      onSaved({ ...entry, title, alias: aliasList, tags: tagList, content });
    } finally {
      setSaving(false);
    }
  }

  return (
    <Card className="flex h-full min-h-0 flex-col">
      <CardHeader className="pb-3">
        <CardTitle className="text-base">Edit entry</CardTitle>
        <div className="grid gap-2">
          <div className="grid gap-1">
            <Label htmlFor="wiki-title" className="text-xs text-muted-foreground">
              Title (unique, trimmed)
            </Label>
            <Input
              id="wiki-title"
              value={title}
              onChange={(event) => setTitle(event.target.value)}
            />
          </div>
          <div className="grid gap-1">
            <Label htmlFor="wiki-alias" className="text-xs text-muted-foreground">
              Alias (comma separated)
            </Label>
            <Input
              id="wiki-alias"
              value={alias}
              onChange={(event) => setAlias(event.target.value)}
            />
          </div>
          <div className="grid gap-1">
            <Label htmlFor="wiki-tags" className="text-xs text-muted-foreground">
              Tags (comma separated)
            </Label>
            <Input
              id="wiki-tags"
              value={tags}
              onChange={(event) => setTags(event.target.value)}
            />
          </div>
        </div>
      </CardHeader>
      <CardContent className="flex min-h-0 flex-1 flex-col gap-2">
        <textarea
          className="min-h-0 flex-1 resize-none rounded-md border bg-background p-3 font-mono text-sm"
          value={content}
          onChange={(event) => setContent(event.target.value)}
          spellCheck={false}
        />
        <div className="flex justify-end gap-2">
          <Button variant="outline" onClick={onCancel} disabled={saving}>
            Cancel
          </Button>
          <Button onClick={() => void save()} disabled={saving}>
            Save
          </Button>
        </div>
      </CardContent>
    </Card>
  );
}
