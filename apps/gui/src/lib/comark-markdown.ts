/**
 * Shared Comark base for the app's markdown pipelines.
 *
 * Both the agent transcript (`features/agent/agent-markdown.tsx`) and the
 * wiki renderer (`features/wiki/markdown.ts`) build on this module:
 *
 *  - GFM (tables, task lists, fenced code) — CommonMark/GFM base.
 *  - `breaks: true` parity via the breaks plugin (soft breaks → <br>).
 *  - KaTeX math with the same delimiters and whitespace rules as the old
 *    marked extensions (`$…$`, `$$…$$`, `\(…\)`, `\[…\]`; inline `$…$`
 *    must not open or close with whitespace). Comark's math plugin emits
 *    `math` tags; we run KaTeX ourselves in MathML output mode — the
 *    gateway serves the SPA as embedded text assets, so there are no font
 *    files to host and the original TeX stays copyable in
 *    `<semantics><annotation>`.
 *  - The `.md-code` shell (language header + copy button) — views wire
 *    their own delegated `[data-copy-code]` handlers.
 *
 * Feature pipelines extend this with their own plugins (custom markdown-it
 * rules) and components; output is sanitized by the caller through the
 * DOMPurify profile in `lib/markdown`.
 */
import type { ElementNode } from "comark";
import mathPlugin from "comark/plugins/math";
import breaksPlugin from "comark/plugins/breaks";
import katex from "katex";
import { escapeHtml } from "@/lib/markdown";

export const comarkBasePlugins = [mathPlugin({ brackets: true }), breaksPlugin()];

export const comarkBaseComponents = {
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
  // pipeline produced, so view CSS and copy handlers carry over. Only
  // markdown fences (a `language` attr on the pre node) get the shell;
  // raw `<pre>` passthrough keeps its own markup and classes verbatim
  // (e.g. the deck engine's `<pre><code class="language-mermaid">`
  // holders that the lazy mermaid renderer scans for).
  pre: async ([, attrs, codeNode]: ElementNode) => {
    const codeAttrs = Array.isArray(codeNode) && codeNode[1] && typeof codeNode[1] === "object" ? codeNode[1] : {};
    if (attrs?.language === undefined) {
      const cls = codeAttrs.class ? ` class="${escapeHtml(String(codeAttrs.class))}"` : "";
      const text = Array.isArray(codeNode) ? String(codeNode[2] ?? "") : "";
      return `<pre><code${cls}>${escapeHtml(text)}</code></pre>`;
    }
    const language = String(attrs.language).trim().split(/\s+/)[0] ?? "";
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
};

