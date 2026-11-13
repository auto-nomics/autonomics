/** An explicit pre-session editor binding belongs only to that agent's first available session. */
export function editorContextBindingKey(path: string, contextId: string | null): string | null {
  return path ? JSON.stringify([path,contextId]) : null;
}
export function promoteEditorContextBinding(targets: Record<string,string>, path: string, previousContextId: string | null, contextId: string | null): Record<string,string> {
  if (previousContextId !== null || contextId === null) return targets;
  const pending=editorContextBindingKey(path,null),current=editorContextBindingKey(path,contextId);
  if (!pending || !current || !Object.hasOwn(targets,pending)) return targets;
  const next={...targets,[current]:targets[pending]};delete next[pending];return next;
}
