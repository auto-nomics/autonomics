// File IDs identify immutable source snapshots. Excalidraw may normalize SVG
// data URLs in its rendering cache; never export that cache over saved sources.
// User replacements/imports allocate new file IDs, so their new files survive.
export function preserveSourceFiles(scene: string, stored: Record<string, unknown>): string {
  if (!Object.keys(stored).length) return scene;
  const snapshot = JSON.parse(scene) as { files?: Record<string, unknown> };
  snapshot.files = { ...(snapshot.files ?? {}), ...stored };
  return JSON.stringify(snapshot);
}
