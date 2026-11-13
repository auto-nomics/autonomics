/**
 * Agent transcript rendering — Comark edition.
 *
 * The agent message pipeline runs on Comark (`comark` + `@comark/html`),
 * decoupling parsing from rendering. The wiki/deck pipelines stay on the
 * marked factory in `lib/markdown.ts`; this file only replaces what the
 * assistant transcript needs:
 *
 *  - GFM (tables, task lists, fenced code) — CommonMark/GFM base.
 *  - `breaks: true` parity via Comark's breaks plugin (soft breaks → <br>).
 *  - KaTeX math with the same delimiters and whitespace rules as the old
 *    marked extensions (`$…$`, `$$…$$`, `\(…\)`, `\[…\]`; inline `$…$`
 *    must not open or close with whitespace). Comark's math plugin emits
 *    `math` tags; we run KaTeX ourselves in MathML output mode — the
 *    gateway serves the SPA as embedded text assets, so there are no font
 *    files to host and the original TeX stays copyable in
 *    `<semantics><annotation>`.
 *  - The `.md-code` shell (language header + copy button) — the workspace
 *    wires its own delegated `[data-copy-code]` handler.
 *
 * Output is sanitized through the same DOMPurify profile as `lib/markdown`.
 */
import { useEffect, useState } from "react";
import { createHtmlRenderer } from "@comark/html";
import mathPlugin from "comark/plugins/math";
import breaksPlugin from "comark/plugins/breaks";
import DOMPurify from "dompurify";
import katex from "katex";
import { escapeHtml, SANITIZE } from "@/lib/markdown";
import type { ElementNode } from "comark";

const renderer = createHtmlRenderer({
  plugins: [mathPlugin({ brackets: true }), breaksPlugin()],
  components: {
    math: async ([, attrs]: ElementNode) => {
      const cls = String(attrs.class ?? "");
      const display = cls.includes("block");
      const tex = String(attrs.content ?? "").trim();
      if (!tex) return "";
      // Keep the old tokenizer's rule: inline `$…$` with leading/trailing
      // whitespace (as written) is not math — fall back to literal text.
      if (!display && (/^\s/.test(String(attrs.content)) || /\s$/.test(String(attrs.content)))) {
        return escapeHtml(`$${attrs.content}$`);
      }
      const rendered = katex.renderToString(tex, {
        displayMode: display,
        throwOnError: false,
        strict: false,
        // The gateway embeds the SPA as text assets — MathML needs no fonts.
        output: "mathml",
      });
      return `<span class="md-math${display ? " md-math--display" : ""}">${rendered}</span>`;
    },
    // Code shell (language header + copy button) — same markup the marked
    // pipeline produced, so the workspace CSS and copy handler carry over.
    pre: async ([, attrs, codeNode]: ElementNode) => {
      const language = String(attrs?.language ?? "").trim().split(/\s+/)[0] ?? "";
      const text = Array.isArray(codeNode) ? String(codeNode[2] ?? "") : "";
      return [
        '<div class="md-code">',
        '<div class="md-code-head">',
        `<span>${escapeHtml(language)}</span>`,
        '<button type="button" data-copy-code>Copy</button>',
        "</div>",
        `<pre><code>${escapeHtml(text)}</code></pre>`,
        "</div>",
      ].join("");
    },
  },
});

/** Render an assistant message to sanitized HTML (async — Comark decouples
 *  parse from render, so the renderer returns a promise). */
export async function renderAgentMarkdown(text: string): Promise<string> {
  const html = await renderer(text);
  return DOMPurify.sanitize(html, SANITIZE);
}

/**
 * Stream-friendly hook: keeps the last rendered HTML on screen while the
 * next frame renders, so streaming updates never blank the message.
 */
export function useAgentMarkdown(text: string): string {
  const [html, setHtml] = useState("");
  useEffect(() => {
    let cancelled = false;
    renderAgentMarkdown(text).then((next) => {
      if (!cancelled) setHtml(next);
    });
    return () => {
      cancelled = true;
    };
  }, [text]);
  return html;
}
