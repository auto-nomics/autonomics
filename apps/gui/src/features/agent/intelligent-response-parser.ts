import { createSerializedMarkdownParser, type Node } from "comark";
import { textContent } from "comark/utils";
import { renderHtmlFromDocument } from "@comark/html";
import { comarkBaseComponents, comarkBasePlugins } from "@/lib/comark-markdown";
import { sanitizeAgentHtml } from "./agent-markdown";

export type ResponsePart = { kind: "markdown" | "generative-ui"; content: string };

export function createResponseParser() {
  // A parser per message keeps reusable nodes from unrelated streams separate.
  const parse = createSerializedMarkdownParser({ plugins: comarkBasePlugins });
  return async (text: string, streaming: boolean): Promise<ResponsePart[]> => {
    const document = await parse(text, { streaming });
    const parts: ResponsePart[] = [];
    let prose: Node[] = [];
    const flush = async () => {
      if (!prose.length) return;
      const html = await renderHtmlFromDocument({ ...document, nodes: prose }, { components: comarkBaseComponents });
      parts.push({ kind: "markdown", content: sanitizeAgentHtml(html) });
      prose = [];
    };
    for (const node of document.nodes) {
      const language = Array.isArray(node) && node[0] === "pre" ? String(node[1].language ?? "").toLowerCase() : "";
      if (language === "generative-ui") {
        await flush();
        parts.push({ kind: language, content: textContent(node) });
      } else prose.push(node);
    }
    await flush();
    return parts;
  };
}
