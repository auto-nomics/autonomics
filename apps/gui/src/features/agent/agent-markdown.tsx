/**
 * Agent transcript rendering — Comark edition.
 *
 * The agent message pipeline runs on Comark (`comark` + `@comark/html`),
 * decoupling parsing from rendering; the shared math/breaks/code-shell
 * base lives in `lib/comark-markdown`. Output is sanitized through the
 * same DOMPurify profile as `lib/markdown`.
 */
import { useEffect, useState } from "react";
import { createHtmlRenderer } from "@comark/html";
import DOMPurify from "dompurify";
import { comarkBasePlugins, comarkBaseComponents } from "@/lib/comark-markdown";
import { SANITIZE } from "@/lib/markdown";

const renderer = createHtmlRenderer({
  plugins: comarkBasePlugins,
  components: comarkBaseComponents,
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
