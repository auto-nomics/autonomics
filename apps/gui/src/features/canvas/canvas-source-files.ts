// Native serialization keeps only live image files. Keep stored immutable source
// snapshots too: asset modification records refer to them after replacement.
export function preserveSourceFiles(scene: string, stored: Record<string, unknown>): string {
  const snapshot = JSON.parse(scene) as { files?: Record<string, unknown> };
  const files = snapshot.files ?? {};
  const missing = Object.keys(stored).filter(id => !Object.hasOwn(files, id));
  if (!missing.length) return scene;
  snapshot.files = { ...Object.fromEntries(missing.map(id => [id, stored[id]])), ...files };
  return JSON.stringify(snapshot);
}
