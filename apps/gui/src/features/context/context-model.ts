import i18n from "@/i18n";
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
const contextKinds: Record<string, true> = { article: true, evidence: true, knowledge: true, editor: true, image: true, manuscript: true };
// Translated lazily via getters so language switches apply at render time,
// while the record's bracket-access shape stays stable for consumers.
export const contextKindLabel: Record<ContextResource["kind"], string> = {
  get article() { return i18n.t("context:kind.article"); },
  get evidence() { return i18n.t("context:kind.evidence"); },
  get knowledge() { return i18n.t("context:kind.knowledge"); },
  get editor() { return i18n.t("context:kind.editor"); },
  get image() { return i18n.t("context:kind.image"); },
  get manuscript() { return i18n.t("context:kind.manuscript"); },
};

export function articleContext(article: BibArticle): ContextResource {
  return {
    kind: "article", id: article.id, title: article.title,
    scope: i18n.t("context:article.scope"), source: `bib:${article.id}`,
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
      item && typeof item === "object" && Object.hasOwn(contextKinds, item.kind)
      && ["id", "title", "scope", "source", "content"].every(key => typeof item[key] === "string"))) return null;
    return { text: text.slice(0, at), resources };
  } catch { return null; }
}

/** Snapshots are user-provided reference data, never system instructions. */
export function serializeContext(resources: ContextResource[]): string {
  if (!resources.length) return "";
  return `${CONTEXT_MARKER}${JSON.stringify(resources.map(({ kind, id, title, scope, source, revision, content }) => ({ kind, id, title, scope, source, revision, content })), null, 2)}`;
}
