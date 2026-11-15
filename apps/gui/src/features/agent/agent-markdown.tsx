/**
 * Agent transcript rendering — Comark edition.
 *
 * The agent message pipeline runs on Comark (`comark` + `@comark/html`),
 * decoupling parsing from rendering; the shared math/breaks/code-shell
 * base lives in `lib/comark-markdown`. Output is sanitized through the
 * same DOMPurify profile as `lib/markdown`.
 */
import { useEffect, useRef, useState } from "react";
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
  return sanitizeAgentHtml(html);
}

export function sanitizeAgentHtml(html: string): string {
  return DOMPurify.sanitize(html, {
    ...SANITIZE,
    // DOMPurify default safe URI rules, extended only for internal resources.
    ALLOWED_URI_REGEXP: /^(?:(?:wiki|bib):\/\/|(?:(?:f|ht)tps?|mailto|tel|callto|sms|cid|xmpp|matrix):|[^a-z]|[a-z+.-]+(?:[^a-z+.-:]|$))/i,
  });
}

/** While `streaming`, re-parse at most this often (ms); each token still
 *  re-runs the effect, but only the latest text gets parsed. Non-streaming
 *  renders (history loads, edits) parse immediately. */
const STREAM_PARSE_INTERVAL = 100;

/**
 * Stream-friendly hook: keeps the last rendered HTML on screen while the
 * next frame renders, so streaming updates never blank the message. While
 * streaming, parses are trailing-throttled — a full Comark re-parse per
 * token (plus the innerHTML swap and its reflow) dominated the render
 * budget on multi-agent streams.
 */
export function useAgentMarkdown(text: string, streaming = false): string {
  const [html, setHtml] = useState("");
  const lastParse = useRef(0);
  useEffect(() => {
    let cancelled = false;
    const delay = streaming
      ? Math.max(0, STREAM_PARSE_INTERVAL - (Date.now() - lastParse.current))
      : 0;
    const timer = setTimeout(() => {
      if (cancelled) return;
      lastParse.current = Date.now();
      renderAgentMarkdown(text).then((next) => {
        if (!cancelled) setHtml(next);
      });
    }, delay);
    return () => {
      cancelled = true;
      clearTimeout(timer);
    };
  }, [text, streaming]);
  return html;
}
