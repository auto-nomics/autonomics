/**
 * SiYuan-style block editor (编辑模式): the document renders as markdown
 * blocks; the focused block alone flips to a raw-markdown textarea —
 * syntax appears under the cursor, everything else stays rendered.
 *
 * The block list is the source of truth (parsed once per entry); every
 * mutation re-joins and reports the full content string upward, so the
 * parent's dirty tracking and save path see live markdown at all times.
 */
import {
  Bold,
  Braces,
  Code,
  Heading1,
  Heading2,
  Heading3,
  ImagePlus,
  Italic,
  Link2,
  List,
  ListOrdered,
  ListTodo,
  Minus,
  Quote,
  SquareCode,
  Strikethrough,
} from "lucide-react";
import { memo, useCallback, useEffect, useRef, useState, type ReactNode } from "react";
import { toast } from "sonner";

import { Button } from "@/components/ui/button";
import { ScrollArea } from "@/components/ui/scroll-area";
import {
  joinMarkdownBlocks,
  newMarkdownBlock,
  renderWikiMarkdown,
  splitMarkdownBlocks,
  type MarkdownBlock,
} from "./markdown";
import { attachmentRawUrl, uploadAttachment } from "./wiki-api";

interface FocusRequest {
  key: string;
  caret: number | "end";
  caretEnd?: number;
}

const RenderedBlock = memo(function RenderedBlock({
  raw,
  onActivate,
}: {
  raw: string;
  onActivate: () => void;
}) {
  return (
    <div
      className="wiki-block md"
      onMouseDown={(event) => {
        // Copy buttons work in preview — they don't flip the block.
        if (
          event.target instanceof HTMLElement &&
          event.target.closest("[data-copy-code]")
        ) {
          return;
        }
        onActivate();
      }}
      dangerouslySetInnerHTML={{ __html: renderWikiMarkdown(raw) }}
    />
  );
});

function autosize(el: HTMLTextAreaElement) {
  el.style.height = "auto";
  el.style.height = `${el.scrollHeight}px`;
}

export function BlockEditor({
  entryId,
  initialContent,
  onChange,
}: {
  entryId: string;
  initialContent: string;
  onChange: (content: string) => void;
}) {
  const [blocks, setBlocks] = useState<MarkdownBlock[]>(() =>
    splitMarkdownBlocks(initialContent),
  );
  const [editingKey, setEditingKey] = useState<string | null>(null);
  const [focusRequest, setFocusRequest] = useState<FocusRequest | null>(null);
  const textareaRef = useRef<HTMLTextAreaElement | null>(null);
  const fileInput = useRef<HTMLInputElement>(null);

  const applyBlocks = useCallback(
    (next: MarkdownBlock[]) => {
      setBlocks(next);
      onChange(joinMarkdownBlocks(next));
    },
    [onChange],
  );

  const startEdit = useCallback((key: string, caret: number | "end" = "end") => {
    setEditingKey(key);
    setFocusRequest({ key, caret });
  }, []);

  // Mount/focus the editing textarea and pin caret + height.
  useEffect(() => {
    if (!focusRequest) return;
    const el = textareaRef.current;
    if (el) {
      el.focus();
      const caret =
        focusRequest.caret === "end" ? el.value.length : focusRequest.caret;
      const bounded = Math.max(0, Math.min(caret, el.value.length));
      const boundedEnd =
        focusRequest.caretEnd === undefined
          ? bounded
          : Math.max(0, Math.min(focusRequest.caretEnd, el.value.length));
      el.setSelectionRange(bounded, boundedEnd);
      autosize(el);
    }
    setFocusRequest(null);
  }, [focusRequest, editingKey]);

  const updateRaw = (key: string, raw: string) => {
    applyBlocks(blocks.map((block) => (block.key === key ? { ...block, raw } : block)));
  };

  /** Insert a fresh block below `index` (-1 = front) and start editing it. */
  const insertBlockBelow = (index: number, raw: string, caret: number | "end") => {
    const fresh = newMarkdownBlock(raw);
    const next = [...blocks];
    if (index >= 0 && next[index] && !next[index].raw.endsWith("\n")) {
      next[index] = { ...next[index], raw: `${next[index].raw}\n` };
    }
    next.splice(index + 1, 0, fresh);
    applyBlocks(next);
    setEditingKey(fresh.key);
    setFocusRequest({ key: fresh.key, caret });
  };

  const removeBlock = (index: number) => {
    const next = blocks.filter((_, i) => i !== index);
    const fallback = next.length === 0 ? [newMarkdownBlock("")] : next;
    applyBlocks(fallback);
    const target = fallback[Math.min(Math.max(index - 1, 0), fallback.length - 1)];
    if (target) startEdit(target.key, "end");
  };

  const mergeWithPrevious = (index: number) => {
    const prev = blocks[index - 1];
    const current = blocks[index];
    const joined = prev.raw === "" || prev.raw.endsWith("\n")
      ? prev.raw + current.raw
      : `${prev.raw}\n${current.raw}`;
    applyBlocks(
      blocks
        .map((block, i) => (i === index - 1 ? { ...block, raw: joined } : block))
        .filter((_, i) => i !== index),
    );
    setEditingKey(prev.key);
    setFocusRequest({ key: prev.key, caret: prev.raw.length });
  };

  /** Apply a selection-aware transform to the editing block's raw text. */
  const withSelection = (
    fn: (raw: string, start: number, end: number) => { raw: string; caret: number; caretEnd?: number },
  ) => {
    const el = textareaRef.current;
    const key = editingKey;
    if (!el || !key) {
      toast.info("先点击一个块,再使用格式工具");
      return;
    }
    const result = fn(el.value, el.selectionStart, el.selectionEnd);
    updateRaw(key, result.raw);
    setFocusRequest({ key, caret: result.caret, caretEnd: result.caretEnd });
  };

  const wrap = (marker: string, close = marker) =>
    withSelection((raw, start, end) => {
      const selected = raw.slice(start, end);
      const before = raw.slice(0, start);
      const after = raw.slice(end);
      if (
        selected.length >= marker.length + close.length &&
        selected.startsWith(marker) &&
        selected.endsWith(close)
      ) {
        const stripped = selected.slice(marker.length, selected.length - close.length);
        return { raw: before + stripped + after, caret: start, caretEnd: start + stripped.length };
      }
      return {
        raw: before + marker + selected + close + after,
        caret: start + marker.length,
        caretEnd: start + marker.length + selected.length,
      };
    });

  const setHeading = (level: 1 | 2 | 3) => {
    const prefix = `${"#".repeat(level)} `;
    withSelection((raw, start, end) => {
      const lineStart = raw.lastIndexOf("\n", start - 1) + 1;
      const searchEnd = raw.indexOf("\n", end);
      const lineEnd = searchEnd === -1 ? raw.length : searchEnd;
      const line = raw.slice(lineStart, lineEnd);
      const stripped = line.replace(/^#{1,6}\s*/, "");
      const nextLine = line.startsWith(prefix) ? stripped : prefix + stripped;
      return {
        raw: raw.slice(0, lineStart) + nextLine + raw.slice(lineEnd),
        caret: lineStart + nextLine.length,
      };
    });
  };

  const togglePrefix = (prefix: string) =>
    withSelection((raw, start, end) => {
      const lineStart = raw.lastIndexOf("\n", start - 1) + 1;
      const searchEnd = raw.indexOf("\n", end);
      const lineEnd = searchEnd === -1 ? raw.length : searchEnd;
      const line = raw.slice(lineStart, lineEnd);
      const nextLine = line.startsWith(prefix) ? line.slice(prefix.length) : prefix + line;
      return {
        raw: raw.slice(0, lineStart) + nextLine + raw.slice(lineEnd),
        caret: lineStart + nextLine.length,
      };
    });

  const insertSnippet = (text: string, caretOffset?: number) =>
    withSelection((raw, start, end) => ({
      raw: raw.slice(0, start) + text + raw.slice(end),
      caret: start + (caretOffset ?? text.length),
    }));

  const appendBlock = (raw: string) => {
    const next = [...blocks];
    const last = next[next.length - 1];
    if (last && !last.raw.endsWith("\n")) {
      next[next.length - 1] = { ...last, raw: `${last.raw}\n` };
    }
    next.push(newMarkdownBlock(raw));
    applyBlocks(next);
  };

  const editingIndex = blocks.findIndex((block) => block.key === editingKey);

  // Rendered code blocks keep a working copy button (mirrors the reader).
  const onContainerClick = (event: React.MouseEvent<HTMLDivElement>) => {
    const hit = event.target instanceof HTMLElement ? event.target : null;
    const copyButton = hit?.closest("[data-copy-code]");
    if (!(copyButton instanceof HTMLElement)) return;
    const code = copyButton.closest(".md-code")?.querySelector("pre");
    if (!code) return;
    void navigator.clipboard.writeText(code.textContent ?? "");
    copyButton.textContent = "已复制";
    window.setTimeout(() => {
      copyButton.textContent = "Copy";
    }, 1400);
  };

  const onKeyDown = (event: React.KeyboardEvent<HTMLTextAreaElement>, index: number) => {
    const el = event.currentTarget;
    const raw = el.value;

    if ((event.ctrlKey || event.metaKey) && !event.shiftKey) {
      const key = event.key.toLowerCase();
      if (key === "enter") {
        event.preventDefault();
        insertBlockBelow(index, "", "end");
        return;
      }
      if (key === "b") {
        event.preventDefault();
        wrap("**");
        return;
      }
      if (key === "i") {
        event.preventDefault();
        wrap("*");
        return;
      }
      if (key === "k") {
        event.preventDefault();
        insertSnippet("[显示文字](https://)", 1);
        return;
      }
      return; // Ctrl+S etc. is handled at the editor-view level.
    }

    if (event.key === "Escape") {
      event.preventDefault();
      setEditingKey(null);
      el.blur();
      return;
    }
    if (event.key === "Tab") {
      event.preventDefault();
      insertSnippet("  ");
      return;
    }
    if (event.key === "Enter" && !event.shiftKey) {
      // Enter on an empty line closes the block and opens a fresh one —
      // SiYuan's "Enter ends the block" feel without breaking multi-line
      // blocks (lists, quotes keep inserting plain newlines).
      const caret = el.selectionStart;
      const lineStart = raw.lastIndexOf("\n", caret - 1) + 1;
      const rest = raw.slice(el.selectionEnd);
      if (lineStart === caret && (rest === "" || rest.startsWith("\n"))) {
        event.preventDefault();
        insertBlockBelow(index, "", "end");
      }
      return;
    }
    if (event.key === "Backspace" && el.selectionStart === 0 && el.selectionEnd === 0) {
      if (raw.trim() === "" && blocks.length > 1) {
        event.preventDefault();
        removeBlock(index);
      } else if (index > 0) {
        event.preventDefault();
        mergeWithPrevious(index);
      }
      return;
    }
    if (event.key === "ArrowUp" && el.selectionStart === 0 && el.selectionEnd === 0 && index > 0) {
      event.preventDefault();
      startEdit(blocks[index - 1].key, "end");
      return;
    }
    if (
      event.key === "ArrowDown" &&
      el.selectionEnd === raw.length &&
      index < blocks.length - 1
    ) {
      event.preventDefault();
      startEdit(blocks[index + 1].key, 0);
    }
  };

  const tool = (title: string, icon: ReactNode, action: () => void, needsBlock = true) => (
    <Button
      key={title}
      type="button"
      size="icon-sm"
      variant="ghost"
      title={title}
      aria-label={title}
      disabled={needsBlock && editingKey === null}
      onClick={() => action()}
    >
      {icon}
    </Button>
  );

  const separator = <span className="mx-1 h-4 w-px shrink-0 bg-border" />;

  return (
    <div className="flex min-h-0 flex-1 flex-col gap-2">
      <input
        ref={fileInput}
        type="file"
        accept="image/*"
        className="hidden"
        onChange={(event) => {
          const file = event.target.files?.[0];
          event.target.value = "";
          if (!file) return;
          void (async () => {
            try {
              const meta = await uploadAttachment(entryId, file);
              const id = meta.attachment_id ?? meta.id ?? "";
              const markdown = `![${file.name}](${attachmentRawUrl(entryId, id)})`;
              if (editingKey) insertSnippet(`\n${markdown}`);
              else appendBlock(`\n${markdown}`);
              toast.success(`已上传并插入 ${file.name}`);
            } catch (error) {
              toast.error((error as Error).message);
            }
          })();
        }}
      />
      <div className="wiki-toolbar flex flex-wrap items-center gap-0.5 rounded-md border bg-muted/30 px-1 py-1">
        {tool("一级标题", <Heading1 className="h-4 w-4" />, () => setHeading(1))}
        {tool("二级标题", <Heading2 className="h-4 w-4" />, () => setHeading(2))}
        {tool("三级标题", <Heading3 className="h-4 w-4" />, () => setHeading(3))}
        {separator}
        {tool("粗体 Ctrl+B", <Bold className="h-4 w-4" />, () => wrap("**"))}
        {tool("斜体 Ctrl+I", <Italic className="h-4 w-4" />, () => wrap("*"))}
        {tool("删除线", <Strikethrough className="h-4 w-4" />, () => wrap("~~"))}
        {tool("行内代码", <Code className="h-4 w-4" />, () => wrap("`"))}
        {separator}
        {tool("引用", <Quote className="h-4 w-4" />, () => togglePrefix("> "))}
        {tool("无序列表", <List className="h-4 w-4" />, () => togglePrefix("- "))}
        {tool("有序列表", <ListOrdered className="h-4 w-4" />, () => togglePrefix("1. "))}
        {tool("任务列表", <ListTodo className="h-4 w-4" />, () => togglePrefix("- [ ] "))}
        {separator}
        {tool("链接 Ctrl+K", <Link2 className="h-4 w-4" />, () => insertSnippet("[显示文字](https://)", 1))}
        {tool("双链 [[词条]]", <Braces className="h-4 w-4" />, () => insertSnippet("[[|]]", 2))}
        {tool(
          "代码块",
          <SquareCode className="h-4 w-4" />,
          () => insertBlockBelow(editingIndex, "```\n\n```\n", 4),
          false,
        )}
        {tool("分割线", <Minus className="h-4 w-4" />, () => appendBlock("---\n"), false)}
        {tool(
          "上传并插入图像",
          <ImagePlus className="h-4 w-4" />,
          () => fileInput.current?.click(),
          false,
        )}
        <span className="flex-1" />
        <span className="px-1 text-[11px] leading-tight text-muted-foreground">
          点击块编辑 · 空行 Enter 结块 · Ctrl+Enter 新块 · Ctrl+B/I/K
        </span>
      </div>
      <ScrollArea className="min-h-0 flex-1 pr-3">
        <div className="flex flex-col gap-1.5 pb-16" onClick={onContainerClick}>
          {blocks.map((block, index) =>
            block.key === editingKey ? (
              <div key={block.key} className="wiki-block wiki-block--editing">
                <textarea
                  ref={textareaRef}
                  value={block.raw}
                  rows={1}
                  spellCheck={false}
                  aria-label="块内容(markdown 源文)"
                  onChange={(event) => {
                    updateRaw(block.key, event.target.value);
                    autosize(event.target);
                  }}
                  onKeyDown={(event) => onKeyDown(event, index)}
                />
              </div>
            ) : (
              <RenderedBlock
                key={block.key}
                raw={block.raw}
                onActivate={() => startEdit(block.key)}
              />
            ),
          )}
        </div>
      </ScrollArea>
    </div>
  );
}
