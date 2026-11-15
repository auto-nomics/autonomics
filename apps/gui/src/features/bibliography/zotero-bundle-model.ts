export type ZoteroObject = Record<string, unknown>;
export type ZoteroChild = { key: string; data: ZoteroObject };
export type ZoteroRecord = { index: number; key: string; aliases: string[]; notes: ZoteroChild[]; attachments: ZoteroChild[]; collections: string[] };
export type ZoteroCollection = { key: string; name: string; parent: string | null; items: string[] };
export type ZoteroBundle = { records: ZoteroRecord[]; collections: ZoteroCollection[]; warnings: string[] };
const object = (value: unknown): ZoteroObject => value && typeof value === "object" && !Array.isArray(value) ? value as ZoteroObject : {};
const array = (value: unknown): unknown[] => Array.isArray(value) ? value : [];
const text = (value: unknown): string => typeof value === "string" || typeof value === "number" ? String(value) : "";
const data = (value: unknown): ZoteroObject => { const raw = object(value); return raw.data ? object(raw.data) : raw; };
function aliases(value: unknown): string[] {
  const raw = object(value), item = data(value);
  return [...new Set([raw.key, item.key, item.itemKey, item.itemID, item.id, item.uri].map(text).filter(Boolean))];
}
/** Never resolves an exported path against the server or host filesystem. */
export function safeBundlePath(path: string): string | null {
  const normalized = path.replace(/\\/g, "/");
  if (!normalized || normalized.startsWith("/") || /^[a-z][a-z\d+.-]*:/i.test(normalized) || normalized.split("/").some(part => part === ".." || part === "")) return null;
  return normalized.split("/").filter(part => part !== ".").join("/") || null;
}
export function matchBundleFile<T extends { name: string; webkitRelativePath?: string }>(path: string, files: readonly T[]): T | null {
  const expected = safeBundlePath(path);
  if (!expected) return null;
  const matches = files.filter(file => {
    const actual = safeBundlePath(file.webkitRelativePath || file.name);
    return actual === expected || (file.webkitRelativePath && actual?.split("/").slice(1).join("/") === expected);
  });
  if (matches.length > 1) throw new Error(`Ambiguous attachment path: ${path}`);
  return matches[0] || null;
}
export function parseZoteroBundle(contents: string): ZoteroBundle {
  const raw: unknown = JSON.parse(contents), root = object(raw);
  const items = Array.isArray(raw) ? raw : array(root.items);
  if (!items.length) throw new Error("Expected Zotero JSON items.");
  const warnings: string[] = [], records: ZoteroRecord[] = [], byAlias = new Map<string, ZoteroRecord>();
  for (const value of items) {
    const item = data(value);
    if (["note", "attachment", "annotation"].includes(text(item.itemType))) continue;
    const names = aliases(value), key = names[0] || `record-${records.length}`;
    const child = (value: unknown, index: number, kind: string): ZoteroChild => ({ key: aliases(value)[0] || `${key}:${kind}:${index}`, data: data(value) });
    const record: ZoteroRecord = {
      index: records.length, key, aliases: names, collections: array(item.collections).map(text).filter(Boolean),
      notes: array(item.notes).map((value, index) => child(typeof value === "string" ? { note: value } : value, index, "note")),
      attachments: array(item.attachments).map((value, index) => child(value, index, "attachment")),
    };
    records.push(record);
    for (const alias of names) {
      if (byAlias.has(alias)) throw new Error(`Duplicate source item identity: ${alias}`);
      byAlias.set(alias, record);
    }
  }
  for (const value of items) {
    const item = data(value), kind = text(item.itemType);
    if (!["note", "attachment", "annotation"].includes(kind)) continue;
    if (kind === "annotation") { warnings.push("PDF annotations are not migrated; standalone notes are imported as plain text."); continue; }
    const parent = byAlias.get(text(item.parentItem || item.parentKey));
    if (!parent) { warnings.push(`Unattached ${kind} has no matching parent and will be skipped.`); continue; }
    parent[kind === "note" ? "notes" : "attachments"].push({ key: aliases(value)[0] || `${parent.key}:${kind}:${JSON.stringify(item)}`, data: item });
  }
  const collections: ZoteroCollection[] = [];
  const addCollection = (rawValue: unknown, fallback: string, parent: string | null, depth = 0) => {
    if (depth > 64 || collections.length >= 2000) throw new Error("Collection hierarchy exceeds migration limits.");
    const item = data(rawValue), key = aliases(rawValue)[0] || fallback;
    const children = array(item.children), name = text(item.name);
    if (!name.trim()) { warnings.push(`Collection ${key} has no name and will be skipped.`); return; }
    if (collections.some(collection => collection.key === key)) throw new Error(`Duplicate source collection identity: ${key}`);
    const itemRefs = array(item.items).map(value => typeof value === "object" ? text(object(value).id || object(value).key) : text(value));
    itemRefs.push(...children.filter(value => text(object(value).type) === "item").map(value => text(object(value).id || object(value).key)));
    collections.push({ key, name, parent: text(item.parentCollection) || parent, items: itemRefs.filter(Boolean) });
    children.filter(value => text(object(value).type) !== "item").forEach((value, index) => addCollection(value, `${key}/${index}`, key, depth + 1));
  };
  const sourceCollections = root.collections;
  if (Array.isArray(sourceCollections)) sourceCollections.forEach((value, index) => addCollection(value, `collection-${index}`, null));
  else for (const [key, value] of Object.entries(object(sourceCollections))) addCollection(value, key, null);
  const keys = new Set(collections.map(collection => collection.key));
  for (const collection of collections) {
    if (collection.parent && !keys.has(collection.parent)) throw new Error(`Missing parent collection: ${collection.parent}`);
    const visited = new Set<string>(); let current: ZoteroCollection | undefined = collection;
    while (current) {
      if (visited.has(current.key)) throw new Error("Collection hierarchy contains a cycle.");
      visited.add(current.key); current = collections.find(candidate => candidate.key === current!.parent);
    }
  }
  for (const record of records) for (const key of record.collections) if (!keys.has(key)) warnings.push(`Collection ${key} was not supplied; its membership cannot be restored.`);
  if (records.length > 2000) throw new Error("Import exceeds 2,000 records.");
  return { records, collections, warnings: [...new Set(warnings)] };
}

export function zoteroMetadataWarnings(records: Array<{ article?: { title?: string }; warnings?: string[] }>): string[] {
  return [...new Set(records.flatMap((record, index) => (record.warnings || []).map(warning => `${record.article?.title || `Record ${index + 1}`}: ${warning}`)))];
}
