import DOMPurify from "dompurify";
import { marked } from "marked";

export function escapeHtml(value: unknown): string {
  return String(value)
    .replaceAll("&", "&amp;")
    .replaceAll("<", "&lt;")
    .replaceAll(">", "&gt;")
    .replaceAll('"', "&quot;")
    .replaceAll("'", "&#39;");
}

export function renderMarkdown(text: string): string {
  return DOMPurify.sanitize(marked.parse(text, { async: false, gfm: true, breaks: true }), {
    USE_PROFILES: { html: true },
  });
}
