export type ProjectFileChange = { path: string; type: 1 | 2 | 3 };
const subscribers = new Map<string, Set<(changes: ProjectFileChange[]) => void>>();

export function subscribeProjectChanges(project: string, listener: (changes: ProjectFileChange[]) => void) {
  let group = subscribers.get(project);
  if (!group) { group = new Set(); subscribers.set(project, group); }
  group.add(listener);
  return () => { group?.delete(listener); if (!group?.size) subscribers.delete(project); };
}

export function publishProjectChanges(project: string, changes: ProjectFileChange[]) {
  if (changes.length) subscribers.get(project)?.forEach(listener => listener(changes));
}
