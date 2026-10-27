import DOMPurify from "dompurify";
import { marked, type Tokens } from "marked";

export function escapeHtml(value: unknown): string {
  return String(value)
    .replaceAll("&", "&amp;")
    .replaceAll("<", "&lt;")
    .replaceAll(">", "&gt;")
    .replaceAll('"', "&quot;")
    .replaceAll("'", "&#39;");
}

marked.use({
  renderer: {
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

export function renderMarkdown(text: string): string {
  return DOMPurify.sanitize(marked.parse(text, { async: false, gfm: true, breaks: true }), {
    USE_PROFILES: { html: true },
  });
}
