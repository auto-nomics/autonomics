import { parseContextMessage, resourceKey, serializeContext, type ContextResource } from "./context-model";
function combineResources(first: ContextResource[], second: ContextResource[]) {
  const resources = new Map(first.map(resource => [resourceKey(resource), resource]));
  for (const resource of second) resources.set(resourceKey(resource), resource);
  return [...resources.values()];
}
/** Keep request attachments in the same draft transaction as its visible text. */
export function appendDraftContext(previous: string, request: string, resource: ContextResource): string {
  const parsed = parseContextMessage(previous);
  return [(parsed?.text ?? previous).trim(), request.trim()].filter(Boolean).join("\n\n") + serializeContext(combineResources(parsed?.resources ?? [], [resource]));
}
/** Merge composer attachments with explicit session references into one envelope. */
export function mergeDraftContext(draft: string, context: string): string {
  if (!context) return draft;
  const extra = parseContextMessage(context);
  if (!extra) return draft + context;
  const prepared = parseContextMessage(draft);
  return (prepared?.text ?? draft) + extra.text + serializeContext(combineResources(prepared?.resources ?? [], extra.resources));
}
/** A failed POST must retain its request even if the user began the next draft. */
export function restoreFailedContextDraft(current: string, sent: string): string {
  if (!current.trim()) return sent;
  if (current === sent) return current;
  const newer = parseContextMessage(current), older = parseContextMessage(sent);
  const oldText = (older?.text ?? sent).trim(), newText = (newer?.text ?? current).trim();
  return (oldText === newText ? newText : [oldText, newText].filter(Boolean).join("\n\n")) + serializeContext(combineResources(older?.resources ?? [], newer?.resources ?? []));
}

/** Legacy handoffs may append text while retaining prepared request attachments. */
export function appendDraftText(previous: string, text: string): string {
  const parsed = parseContextMessage(previous);
  return [(parsed?.text ?? previous).trim(), text.trim()].filter(Boolean).join("\n\n") + serializeContext(parsed?.resources ?? []);
}
