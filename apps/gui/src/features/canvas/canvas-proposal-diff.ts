/** Review only meaningful scene changes; never render embedded image bytes. */
type Row = Record<string, unknown>;
const row = (value: unknown): Row => value && typeof value === "object" && !Array.isArray(value) ? value as Row : {};
const bookkeeping = new Set(["version", "versionNonce", "updated"]);
function stable(value: unknown): string {
  if (value === undefined) return "undefined";
  if (Array.isArray(value)) return `[${value.map(stable).join(",")}]`;
  if (value && typeof value === "object") return `{${Object.keys(value).sort().map(key => `${JSON.stringify(key)}:${stable(row(value)[key])}`).join(",")}}`;
  return JSON.stringify(value);
}
function redact(value: unknown): unknown {
  if (typeof value === "string" && value.startsWith("data:")) return "（内嵌数据）";
  if (Array.isArray(value)) return value.map(redact);
  if (value && typeof value === "object") return Object.fromEntries(Object.entries(value).map(([key, item]) => [key, key.toLowerCase().includes("dataurl") ? "（内嵌数据）" : redact(item)]));
  return value;
}
function display(value: unknown): string {
  if (value === undefined) return "（无）";
  if (typeof value === "string" && value.startsWith("data:")) return "（内嵌数据）";
  const text = typeof value === "string" ? value : stable(redact(value));
  return text.length > 600 ? `${text.slice(0,600)}…（已截断）` : text;
}
export type FieldChange = { field: string; before: string; after: string };
export type ObjectChange = { id: string; type: string; kind: "added" | "removed" | "changed"; fields: FieldChange[]; fieldsTruncated: boolean };
function documentAttributeDiff(before: Row, after: Row) {
  const keys = [...new Set([...Object.keys(before), ...Object.keys(after)])].filter(key => !["elements", "files", "appState"].includes(key) && stable(before[key]) !== stable(after[key]));
  const fields = keys.slice(0, 40).map(field => ({field, before:field.toLowerCase().includes("dataurl") ? "（内嵌数据）" : display(before[field]), after:field.toLowerCase().includes("dataurl") ? "（内嵌数据）" : display(after[field])}));
  return {documentFields:fields, documentFieldCount:keys.length, documentFieldsTruncated:keys.length>fields.length};
}
/** Read-only review summary; never a document replacement or scientific validation. */
export function documentAttributeFeedback(before: unknown, after: unknown) {
  const diff=documentAttributeDiff(row(before),row(after));
  const fields: (FieldChange & {text_truncated:boolean})[]=[];
  for(const value of diff.documentFields.slice(0,10)) {
    const field={...value,before:value.before.slice(0,300),after:value.after.slice(0,300),text_truncated:value.before.length>300||value.after.length>300};
    if(new TextEncoder().encode(JSON.stringify([...fields,field])).length>4096)break;
    fields.push(field);
  }
  return {scope:"document_attributes",total:diff.documentFieldCount,omitted:diff.documentFieldCount-fields.length,fields};
}
export function proposalDiff(beforeInput: unknown, afterInput: unknown) {
  const before = row(beforeInput), after = row(afterInput);
  const index = (scene: Row) => new Map((Array.isArray(scene.elements) ? scene.elements : []).map(row).filter(e => e.isDeleted !== true && typeof e.id === "string").map(e => [String(e.id), e]));
  const a = index(before), b = index(after), objects: ObjectChange[] = [];
  let added = 0, removed = 0, changed = 0;
  for (const id of new Set([...a.keys(), ...b.keys()])) {
    const left = a.get(id), right = b.get(id);
    const fields: FieldChange[] = []; let fieldCount = 0;
    const visit = (x: unknown, y: unknown, path: string) => {
      if (stable(x) === stable(y)) return;
      if (x && y && typeof x === "object" && typeof y === "object" && !Array.isArray(x) && !Array.isArray(y)) {
        for (const key of new Set([...Object.keys(row(x)), ...Object.keys(row(y))])) visit(row(x)[key],row(y)[key],path ? `${path}.${key}` : key);
      } else {
        fieldCount++;
        if (fields.length < 40) fields.push({field:path,before:path.toLowerCase().includes("dataurl") ? "（内嵌数据）" : display(x),after:path.toLowerCase().includes("dataurl") ? "（内嵌数据）" : display(y)});
      }
    };
    for (const key of new Set([...Object.keys(left ?? {}),...Object.keys(right ?? {})])) {
      if (key !== "id" && key !== "isDeleted" && !bookkeeping.has(key)) visit(left?.[key],right?.[key],key);
    }
    const kind = !left ? "added" : !right ? "removed" : fieldCount ? "changed" : null;
    if (!kind) continue;
    if (kind === "added") added++; else if (kind === "removed") removed++; else changed++;
    if (objects.length < 200) objects.push({id,type:String((right ?? left)?.type ?? "object"),kind,fields,fieldsTruncated:fieldCount>fields.length});
  }
  const filesA = row(before.files), filesB = row(after.files);
  const files = [...new Set([...Object.keys(filesA),...Object.keys(filesB)])].filter(id => stable(filesA[id]) !== stable(filesB[id])).map(id => ({id,kind:!(id in filesA) ? "added" : !(id in filesB) ? "removed" : "changed"}));
  return {added,removed,changed,objects,truncated:added+removed+changed>objects.length,files:files.slice(0,100),filesTruncated:files.length>100,fileCount:files.length,
    orderChanged: stable([...a.keys()].filter(id=>b.has(id))) !== stable([...b.keys()].filter(id=>a.has(id))),
    appStateChanged:stable(before.appState)!==stable(after.appState), ...documentAttributeDiff(before,after)};
}
