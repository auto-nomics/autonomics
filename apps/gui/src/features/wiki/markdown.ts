/**
 * Wiki markdown pipeline: a private `Marked` instance (the shared one in
 * lib/markdown must stay untouched for the agent transcript) with two
 * wiki-specific behaviours:
 *
 * - `[[target]]` / `[[target|display]]` wikilinks render as inert
 *   `<a data-wikilink>` chips — click routing stays in React.
 * - the lexer powers the SiYuan-style block editor: content splits into
 *   top-level markdown blocks whose `raw` fragments re-join losslessly,
 *   so "blocks ⇄ content" never rewrites the document.
 */
import DOMPurify from "dompurify";
import { Marked, type Tokens } from "marked";

import { escapeHtml } from "@/lib/markdown";

const WIKILINK_TOKEN_RE = /^\[\[([^\[\]\n]+)\]\]/;
const WIKILINK_SCAN_RE = /\[\[([^\[\]\n]+)\]\]/g;

interface WikilinkToken extends Tokens.Generic {
  target: string;
  display: string;
}

const wikiMarked = new Marked();

wikiMarked.use({
  gfm: true,
  breaks: true,
  extensions: [
    {
      name: "wikilink",
      level: "inline",
      start(src: string) {
        const index = src.indexOf("[[");
        return index === -1 ? undefined : index;
      },
      tokenizer(src: string): WikilinkToken | undefined {
        const match = WIKILINK_TOKEN_RE.exec(src);
        if (!match) return undefined;
        const pipe = match[1].indexOf("|");
        const target = (pipe === -1 ? match[1] : match[1].slice(0, pipe)).trim();
        const display = (pipe === -1 ? match[1] : match[1].slice(pipe + 1)).trim();
        if (!target) return undefined;
        return { type: "wikilink", raw: match[0], target, display: display || target };
      },
      renderer(token: Tokens.Generic) {
        const link = token as WikilinkToken;
        return `<a data-wikilink="${escapeHtml(link.target)}" class="md-wikilink">${escapeHtml(link.display)}</a>`;
      },
    },
  ],
  renderer: {
    // Same shell as lib/markdown (language header + copy button) — the
    // wiki views wire their own delegated copy handler.
    code(token: Tokens.Code) {
      const language = (token.lang ?? "").trim().split(/\s+/)[0] ?? "";
      return [
        '<div class="md-code">',
        '<div class="md-code-head">',
        `<span>${escapeHtml(language)}</span>`,
        '<button type="button" data-copy-code>Copy</button>',
        "</div>",
        `<pre><code>${escapeHtml(token.text)}</code></pre>`,
        "</div>",
      ].join("");
    },
  },
});

export function renderWikiMarkdown(text: string): string {
  return DOMPurify.sanitize(wikiMarked.parse(text, { async: false }), {
    USE_PROFILES: { html: true },
  });
}

/** Unique `[[target]]` names in document order (display text ignored). */
export function extractWikiTargets(text: string): string[] {
  const names: string[] = [];
  for (const match of text.matchAll(WIKILINK_SCAN_RE)) {
    const target = match[1].split("|")[0].trim();
    if (target && !names.includes(target)) names.push(target);
  }
  return names;
}

export interface MarkdownBlock {
  key: string;
  raw: string;
  kind: string;
}

let blockSeq = 0;

export function newMarkdownBlock(raw: string, kind = "paragraph"): MarkdownBlock {
  blockSeq += 1;
  return { key: `b${blockSeq}`, raw, kind };
}

/**
 * Split content into top-level markdown blocks. Guarded for losslessness:
 * if the lexer ever drops or rewrites text (unexpected token shape), the
 * whole document degrades to one editable block instead of corrupting it.
 */
export function splitMarkdownBlocks(content: string): MarkdownBlock[] {
  const tokens = wikiMarked.lexer(content);
  const blocks = tokens
    .map((token) => ({ raw: token.raw ?? "", kind: token.type }))
    .filter((block) => block.raw.length > 0)
    .map((block) => newMarkdownBlock(block.raw, block.kind));
  if (blocks.length === 0 || blocks.map((b) => b.raw).join("") !== content) {
    return [newMarkdownBlock(content)];
  }
  return blocks;
}

export function joinMarkdownBlocks(blocks: MarkdownBlock[]): string {
  return blocks.map((block) => block.raw).join("");
}
