/**
 * Wiki markdown surface — Comark edition.
 *
 * Rendering runs on Comark (`comark` + `@comark/html`) over the shared
 * base in `lib/comark-markdown`, plus wiki-only syntax written as
 * markdown-it parser rules (Comark keeps markdown-it's plugin API). The
 * rules emit `html_inline`/`html_block` tokens, which Comark converts to
 * plain AST nodes — custom *token* types have no AST mapping, but raw
 * HTML passes straight through to the renderer (DOMPurify sanitizes the
 * final document either way):
 *
 *  - `[[wikilink]]` chips — click routing stays in React (delegated
 *    handlers look for `a[data-wikilink]`).
 *  - `<!--wiki-image-upload:uuid-->` markers — document-local placeholders
 *    the block editor replaces after upload.
 *  - an `img` component that rewrites `wiki-attach:` refs to API URLs and
 *    keeps the desktop (Tauri) lazy-load contract.
 *
 * The SiYuan-style block editor model keeps the **marked lexer** from
 * `lib/markdown`: the block splitter needs each top-level block's raw
 * source fragment so "blocks ⇄ content" re-joins losslessly — Comark's
 * AST is a compact semantic tree and does not preserve raw spans, so the
 * lexer stays marked while rendering moves to Comark.
 */
import { useEffect, useState } from "react";
import i18n from "@/i18n";
import { renderHtml } from "@comark/html";
import { defineComarkPlugin } from "comark";
import DOMPurify from "dompurify";
import { comarkBasePlugins, comarkBaseComponents } from "@/lib/comark-markdown";
import { escapeHtml, SANITIZE, wikiLexer } from "@/lib/markdown";
import { wikiAttachmentRef, wikiAttachmentUrl } from "@/lib/wiki-attachment-ref";
import type { ElementNode } from "comark";
import type { RuleInline, RuleBlock, StateInline, StateBlock, Token, MarkdownExit } from "markdown-exit/core";

// ── wikilinks ─────────────────────────────────────────────────────────

const WIKILINK_SCAN_RE = /\[\[([^\[\]\n]+)\]\]/g;
const WIKILINK_TOKEN_RE = /^\[\[([^\[\]\n]+)\]\]/;

/**
 * `[[target]]` / `[[target|display]]` chips. Registered before `escape`
 * — markdown-exit natively parses `[[…]]` as its inline-span component
 * syntax (`comark_inline_span`), so the rule must run before that too.
 * The display text goes through a `text` token (renderer-escaped); the
 * target is attribute-escaped here since it lands in a raw HTML token.
 */
const wikilinkPlugin = defineComarkPlugin(() => ({
  name: "wikilink",
  markdownItPlugins: [
    (md: MarkdownExit) => {
      md.inline.ruler.before("escape", "wikilink", ((state: StateInline, silent: boolean) => {
        const match = WIKILINK_TOKEN_RE.exec(state.src.slice(state.pos, state.posMax));
        if (!match) return false;
        const pipe = match[1].indexOf("|");
        const target = (pipe === -1 ? match[1] : match[1].slice(0, pipe)).trim();
        const display = ((pipe === -1 ? match[1] : match[1].slice(pipe + 1)).trim() || target);
        if (!target) return false;
        if (!silent) {
          let token = state.push("html_inline", "", 0);
          token.content = `<a data-wikilink="${escapeHtml(target)}" class="md-wikilink">`;
          token = state.push("text", "", 0);
          token.content = display;
          token = state.push("html_inline", "", 0);
          token.content = "</a>";
        }
        state.pos += match[0].length;
        return true;
      }) as RuleInline);
    },
  ],
}));

// ── citations ─────────────────────────────────────────────────────────

const CITATION_TOKEN_RE = /^\[\@([A-Za-z0-9_][A-Za-z0-9_.\-]*(?:[ \t]*;[ \t]*\@[A-Za-z0-9_][A-Za-z0-9_.\-]*)*)\]/;

/**
 * Pandoc-style citations `[@smith2024a-k3x9]` / `[@k1; @k2]`. Each key in
 * the group renders as its own chip (`a[data-citekey]`); the bracket glue
 * stays as plain text so the citation reads as one group. Click routing is
 * delegated like wikilinks — see reading-view (the value is a citekey; the
 * bib record endpoint resolves citekeys transparently).
 */
const citationPlugin = defineComarkPlugin(() => ({
  name: "citation",
  markdownItPlugins: [
    (md: MarkdownExit) => {
      md.inline.ruler.before("escape", "citation", ((state: StateInline, silent: boolean) => {
        const match = CITATION_TOKEN_RE.exec(state.src.slice(state.pos, state.posMax));
        if (!match) return false;
        if (!silent) {
          let token = state.push("html_inline", "", 0);
          token.content = '<span class="md-citation-group">';
          for (const raw of match[1].split(";")) {
            const key = raw.trim().replace(/^\@/, "");
            if (!key) continue;
            token = state.push("html_inline", "", 0);
            token.content = `<a data-citekey="${escapeHtml(key)}" class="md-citation">@${escapeHtml(key)}</a>`;
          }
          token = state.push("html_inline", "", 0);
          token.content = "</span>";
        }
        state.pos += match[0].length;
        return true;
      }) as RuleInline);
    },
  ],
}));

// Upload markers are document-local placeholders, replaced by the editor after upload.
const IMAGE_UPLOAD_TOKEN_RE = /^<!--wiki-image-upload:([0-9a-f-]{36})-->[ \t]*$/i;

const imageUploadPlugin = defineComarkPlugin(() => ({
  name: "wiki-image-upload",
  markdownItPlugins: [
    (md: MarkdownExit) => {
      md.block.ruler.before("html_block", "wiki_image_upload", ((state: StateBlock, startLine: number, _endLine: number, silent: boolean) => {
        const start = state.bMarks[startLine] + state.tShift[startLine];
        const max = state.eMarks[startLine];
        const match = IMAGE_UPLOAD_TOKEN_RE.exec(state.src.slice(start, max));
        if (!match) return false;
        if (silent) return true;
        const token: Token = state.push("html_block", "", 0);
        token.content = `<p class="wiki-image-pending" role="status">${i18n.t("wiki:images.pending")}</p>`;
        token.map = [startLine, startLine + 1];
        state.line = startLine + 1;
        return true;
      }) as RuleBlock);
    },
  ],
}));

// ── pipeline ──────────────────────────────────────────────────────────

const wikiPlugins = [...comarkBasePlugins, wikilinkPlugin(), citationPlugin(), imageUploadPlugin()];

function wikiComponents(entryId?: string) {
  return {
    ...comarkBaseComponents,
    img: ([, attrs]: ElementNode) => {
      const src = String(attrs.src ?? "");
      // Comark keeps the alt's raw source (backslash escapes intact);
      // marked parsed it to plain text — unescape for parity.
      const alt = String(attrs.alt ?? "").replace(/\\([\\!"#$%&'()*+,\-./:;<=>?@[\]^_`{|}~])/g, "$1");
      const title = attrs.title ? ` title="${escapeHtml(String(attrs.title))}"` : "";
      const ref = wikiAttachmentRef(src, entryId);
      const url = ref ? wikiAttachmentUrl(ref.entryId, ref.attachmentId) : src;
      if (url.startsWith("wiki-attach:")) {
        return `<span class="wiki-image-error">${i18n.t("wiki:images.invalidRef", { alt: escapeHtml(alt) })}</span>`;
      }
      const desktop = typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;
      return `<img${desktop ? "" : ` src="${escapeHtml(url)}"`} alt="${escapeHtml(alt)}"${title} loading="lazy" decoding="async"${ref ? ` data-wiki-entry="${escapeHtml(ref.entryId)}" data-wiki-attachment="${escapeHtml(ref.attachmentId)}"` : ""}>`;
    },
  };
}

/** Render wiki content to sanitized HTML (async — Comark decouples parse
 *  from render, so the renderer returns a promise). */
export async function renderWikiMarkdown(text: string, entryId?: string): Promise<string> {
  const html = await renderHtml(text, {
    plugins: wikiPlugins,
    components: wikiComponents(entryId),
  });
  return DOMPurify.sanitize(html, SANITIZE);
}

/**
 * Stream/async-friendly hook: keeps the last rendered HTML on screen
 * while the next frame renders. Views that scan the rendered DOM
 * (attachment images, wikilink resolution passes) should key their pass
 * on the returned HTML, not the source text.
 */
export function useWikiMarkdown(text: string, entryId?: string): string {
  const [html, setHtml] = useState("");
  useEffect(() => {
    let cancelled = false;
    renderWikiMarkdown(text, entryId).then((next) => {
      if (!cancelled) setHtml(next);
    });
    return () => {
      cancelled = true;
    };
  }, [text, entryId]);
  return html;
}

// ── backlinks / block editor ──────────────────────────────────────────

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
 *
 * Stays on the marked lexer (see file header): Comark's AST does not keep
 * raw source fragments, and the editor's blocks must re-join losslessly.
 */
export function splitMarkdownBlocks(content: string): MarkdownBlock[] {
  const tokens = wikiLexer(content);
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
