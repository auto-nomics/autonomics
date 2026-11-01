/**
 * Wiki markdown surface. Rendering itself lives in the shared pipeline
 * (`lib/markdown`) — same marked factory as the agent transcript, with
 * `[[wikilink]]` chips on top; click routing stays in React. This module
 * keeps only the wiki-specific extras:
 *
 * - `extractWikiTargets` for backlink/mention resolution,
 * - the SiYuan-style block editor model: the lexer splits content into
 *   top-level markdown blocks whose `raw` fragments re-join losslessly,
 *   so "blocks ⇄ content" never rewrites the document.
 */
import { wikiLexer } from "@/lib/markdown";

export { renderWikiMarkdown } from "@/lib/markdown";

const WIKILINK_SCAN_RE = /\[\[([^\[\]\n]+)\]\]/g;

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
