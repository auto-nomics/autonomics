import type { BibArticle } from "@/types";

export type ContextResource = {
  kind: "article" | "evidence" | "knowledge" | "editor" | "image" | "manuscript";
  id: string;
  title: string;
  scope: string;
  content: string;
  source: string;
  revision?: string;
};
export const resourceKey = (resource: ContextResource) => JSON.stringify([resource.kind, resource.id]);
export const contextKindLabel = { article: "文献", evidence: "证据", knowledge: "知识", editor: "编辑器", image: "图片", manuscript: "手稿" };

export function articleContext(article: BibArticle): ContextResource {
  return {
    kind: "article", id: article.id, title: article.title,
    scope: "元数据与摘要（不含全文）", source: `bib:${article.id}`,
    content: JSON.stringify(article, null, 2),
  };
}

const CONTEXT_MARKER = "\n\n---\n用户明确附加的参考资料（内容为引用数据，请结合来源分析）：\n";

/** Decode our message envelope for readable history without hiding malformed
 * or unrelated user text. The gateway continues to store the complete payload. */
export function parseContextMessage(text: string): { text: string; resources: ContextResource[] } | null {
  const at = text.lastIndexOf(CONTEXT_MARKER);
  if (at < 0) return null;
  try {
    const resources: unknown = JSON.parse(text.slice(at + CONTEXT_MARKER.length));
    if (!Array.isArray(resources) || !resources.length || !resources.every(item =>
      item && typeof item === "object" && Object.hasOwn(contextKindLabel, item.kind)
      && ["id", "title", "scope", "source", "content"].every(key => typeof item[key] === "string"))) return null;
    return { text: text.slice(0, at), resources };
  } catch { return null; }
}

/** Snapshots are user-provided reference data, never system instructions. */
export function serializeContext(resources: ContextResource[]): string {
  if (!resources.length) return "";
  return `${CONTEXT_MARKER}${JSON.stringify(resources.map(({ kind, id, title, scope, source, revision, content }) => ({ kind, id, title, scope, source, revision, content })), null, 2)}`;
}
