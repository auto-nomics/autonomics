import type { SourceFile } from "./types";
export type Draft = { content: string; base: SourceFile };
export const getDraft = (
  drafts: Record<string, Draft>,
  path: string,
): Draft | undefined =>
  Object.hasOwn(drafts, path) ? drafts[path] : undefined;
export const draftStorageKey = (
  gateway: string,
  projectId: string,
  createdAt: string,
) =>
  `autonomics.manuscript-drafts.v1:${JSON.stringify([gateway, projectId, createdAt])}`;
export function readDrafts(key: string): Record<string, Draft> {
  const raw = sessionStorage.getItem(key);
  if (!raw) return Object.create(null);
  const parsed: unknown = JSON.parse(raw);
  if (!parsed || typeof parsed !== "object" || Array.isArray(parsed))
    throw new Error("本地草稿格式无效，未覆盖服务器文件。");
  const result: Record<string, Draft> = Object.create(null);
  for (const [path, entry] of Object.entries(parsed)) {
    const draft = entry as Partial<Draft>;
    if (
      !draft ||
      typeof draft.content !== "string" ||
      !draft.base ||
      draft.base.path !== path ||
      typeof draft.base.content !== "string" ||
      !Number.isSafeInteger(draft.base.revision) ||
      draft.base.revision < 0
    )
      throw new Error("本地草稿格式无效，未覆盖服务器文件。");
    if (draft.content !== draft.base.content) result[path] = draft as Draft;
  }
  return result;
}
export function writeDrafts(key: string, drafts: Record<string, Draft>) {
  if (Object.keys(drafts).length)
    sessionStorage.setItem(key, JSON.stringify(drafts));
  else sessionStorage.removeItem(key);
}
