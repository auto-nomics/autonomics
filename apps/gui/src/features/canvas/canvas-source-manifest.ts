/** Source inventory for the exact exported scene; never includes image bytes or user notes. */
import i18n from "@/i18n";
type ObjectValue = Record<string, unknown>;
const object = (value: unknown): ObjectValue => value && typeof value === "object" && !Array.isArray(value) ? value as ObjectValue : {};
const string = (value: unknown): string | null => typeof value === "string" && value.trim() ? value : null;
const strings = (value: unknown): string[] => Array.isArray(value) ? value.map(string).filter((v): v is string => v !== null) : string(value) ? [string(value)!] : [];
const identifier = (value: unknown) => string(value) ?? (typeof value === "number" && Number.isFinite(value) ? String(value) : null);
const unique = (values: string[]) => [...new Set(values)];
function urls(...values: unknown[]) {
  return unique(values.flatMap(strings).filter(value => { try { const url = new URL(value); return ["http:", "https:"].includes(url.protocol) && !url.username && !url.password; } catch { return false; } }));
}
export async function sha256(bytes: Uint8Array) {
  return [...new Uint8Array(await crypto.subtle.digest("SHA-256", Uint8Array.from(bytes).buffer))].map(v => v.toString(16).padStart(2, "0")).join("");
}
async function fileHash(file: ObjectValue) {
  const dataURL = string(file.dataURL); if (!dataURL) return { actual_sha256: null, file_status: "missing" as const };
  try {
    const match = /^data:([^,]*),([\s\S]*)$/.exec(dataURL); if (!match) throw new Error("invalid data URL");
    const bytes = /;base64(?:;|$)/i.test(match[1]) ? Uint8Array.from(atob(match[2]), character => character.charCodeAt(0)) : new TextEncoder().encode(decodeURIComponent(match[2]));
    return { actual_sha256: await sha256(bytes), file_status: "available" as const };
  } catch { return { actual_sha256: null, file_status: "invalid_encoding" as const }; }
}
function insetChange(row: ObjectValue) {
  if (!["inset", "inset_refresh"].includes(String(row.kind))) return {};
  const rectangle = (value: unknown, keys: string[]) => {
    const rect = object(value);
    return Object.fromEntries(keys.map(key => [key, typeof rect[key] === "number" && Number.isFinite(rect[key]) ? rect[key] : null]));
  };
  return { ...(row.kind === "inset_refresh" ? { previous_source_revision: typeof row.previous_source_revision === "number" && Number.isSafeInteger(row.previous_source_revision) && row.previous_source_revision > 0 ? row.previous_source_revision : null, previous_file_id: string(row.previous_file_id), previous_sha256: string(row.previous_sha256) } : {}), source_element_id: string(row.source_element_id), source_revision: typeof row.source_revision === "number" && Number.isSafeInteger(row.source_revision) && row.source_revision > 0 ? row.source_revision : null, region: rectangle(row.region, ["x", "y", "width", "height"]), crop: rectangle(row.crop, ["x", "y", "width", "height", "naturalWidth", "naturalHeight"]) };
}
function changes(value: unknown) {
  return (Array.isArray(value) ? value : []).map(entry => {
    const row = object(entry);
    return { kind: string(row.kind), ...insetChange(row), ...(["path_style", "path_edit"].includes(String(row.kind)) ? { edits: (Array.isArray(row.edits) ? row.edits : []).map(value => { const edit = object(value); const paint = (v: unknown) => typeof v === "string" && (v === "none" || /^#[a-f\d]{6}$/i.test(v)) ? v : null; return { part_id: string(edit.part_id), ...(Array.isArray(edit.points) ? { points: edit.points.map(value => { const point = object(value); return { index: typeof point.index === "number" && Number.isSafeInteger(point.index) && point.index >= 0 ? point.index : null, position: Array.isArray(point.position) && point.position.length === 2 && point.position.every(v => typeof v === "number" && Number.isFinite(v) && Math.abs(v) <= 1e6) ? point.position as number[] : null }; }) } : {}), ...(Array.isArray(edit.translate) && edit.translate.length === 2 && edit.translate.every(v => typeof v === "number" && Number.isFinite(v) && Math.abs(v) <= 100000) ? { translate: edit.translate as number[] } : {}), fill: paint(edit.fill), stroke: paint(edit.stroke), stroke_width: typeof edit.stroke_width === "number" && Number.isFinite(edit.stroke_width) && edit.stroke_width > 0 && edit.stroke_width <= 1000 ? edit.stroke_width : null, opacity: typeof edit.opacity === "number" && Number.isFinite(edit.opacity) && edit.opacity >= 0 && edit.opacity <= 1 ? edit.opacity : null }; }) } : {}), colors: Object.fromEntries(Object.entries(object(row.colors)).filter(([from, to]) => /^#[a-f\d]{6}$/i.test(from) && typeof to === "string" && /^#[a-f\d]{6}$/i.test(to))), based_on_file_id: string(row.based_on_file_id), based_on_sha256: string(row.based_on_sha256), result_sha256: string(row.result_sha256), created_at: string(row.created_at) };
  });
}
function displayCrop(value: unknown) {
  if (value == null) return null;
  const crop = object(value), keys = ['x','y','width','height','naturalWidth','naturalHeight'] as const;
  if (!keys.every(key => typeof crop[key] === 'number' && Number.isFinite(crop[key]))
    || Number(crop.x)<0 || Number(crop.y)<0 || Number(crop.width)<=0 || Number(crop.height)<=0
    || Number(crop.naturalWidth)<=0 || Number(crop.naturalHeight)<=0
    || Number(crop.x)+Number(crop.width)>Number(crop.naturalWidth)+1e-6
    || Number(crop.y)+Number(crop.height)>Number(crop.naturalHeight)+1e-6) return {status:'invalid_native_crop' as const};
  return {status:'recorded' as const,units:'source_pixels' as const,region:Object.fromEntries(keys.map(key=>[key,crop[key]])),source_file_changed:false};
}
function attribution(data: ObjectValue, fallbackName: string) {
  const asset = object(data.asset);
  const authors = unique([...strings(asset.creators), ...strings(asset.author), ...strings(data.creators), ...strings(data.author)]);
  const licenses = unique([...strings(asset.licenses), ...strings(asset.license), ...strings(data.licenses), ...strings(data.license)]);
  const sourceURLs = urls(asset.source_url, asset.svg_url, data.source_url, data.source, object(data.file).url);
  return { provider: string(data.source), asset_id: identifier(asset.id), name: string(asset.title) ?? string(asset.name) ?? fallbackName, authors, author_urls: urls(asset.author_url), licenses, license_urls: urls(asset.license_url, asset.license_urls, data.license_url, data.license_urls), source_urls: sourceURLs, source_sha256: string(data.source_sha256), recorded_sha256: string(data.sha256), normalization: strings(data.normalization), modifications: changes(data.modifications), missing_fields: [...(!authors.length ? ["authors"] : []), ...(!licenses.length ? ["licenses"] : []), ...(!sourceURLs.length ? ["source_urls"] : [])] };
}
const matchesRecordedHash = (actual: string | null, recorded: string | null) => actual && recorded && /^[a-f\d]{64}$/i.test(recorded) ? actual === recorded.toLowerCase() : null;
async function replacementHistory(value: unknown, lookup: (fileId: string | null) => ReturnType<typeof fileHash>) {
  const recorded = Array.isArray(value), entries = recorded ? value : [];
  const start = Math.max(0, entries.length - 50);
  const exported = await Promise.all(entries.slice(start).map(async (entry, index) => {
    const data = object(entry), previous = data.previous_provenance;
    const previousId = string(data.previous_file_id), resultId = string(data.result_file_id);
    const valid = Boolean(entry && typeof entry === "object" && !Array.isArray(entry) && previous && typeof previous === "object" && !Array.isArray(previous) && previousId && resultId);
    const source = attribution(object(previous), previousId ?? "Previous image"), [hash, resultHash] = await Promise.all([lookup(previousId), lookup(resultId)]);
    return { index: start + index, metadata_status: valid ? "recorded" : "unsupported", previous_file_id: previousId, result_file_id: resultId, result_file: { ...resultHash, hash_matches_file_id: matchesRecordedHash(resultHash.actual_sha256, resultId) }, crop_policy: ["uncropped", "normalized_source_region"].includes(String(data.crop_policy)) ? data.crop_policy as string : null, created_at: string(data.created_at), previous_source: { ...source, ...hash, hash_matches_recorded: matchesRecordedHash(hash.actual_sha256, source.recorded_sha256) } };
  }));
  return { status: value == null ? "not_recorded" : recorded ? "recorded" : "unsupported", verification: "not_verified", entries: exported, start_index: recorded ? start : null, total: recorded ? entries.length : value == null ? 0 : null, truncated: entries.length > exported.length, unsupported_entries_in_export: exported.filter(entry => entry.metadata_status === "unsupported").length };
}
function fileLookup(files: ObjectValue) {
  const hashes = new Map<string | null, ReturnType<typeof fileHash>>();
  return (fileId: string | null) => {
    if (!hashes.has(fileId)) hashes.set(fileId, fileHash(object(fileId ? files[fileId] : undefined)));
    return hashes.get(fileId)!;
  };
}
async function assetRecord(element: ObjectValue, files: ObjectValue, lookup: ReturnType<typeof fileLookup>, all?:ObjectValue[]) {
  const data = object(element.customData), fileId = string(element.fileId), file = object(fileId ? files[fileId] : undefined);
  const source = attribution(data, string(element.id) ?? "Image"), hash = await lookup(fileId);
  const badgeOwner=string(data.semanticBadgeAnchorId),badgeMeaning=typeof data.badgeMeaning==='string'&&!/^\s*data:/i.test(data.badgeMeaning)?data.badgeMeaning.slice(0,512):null;
  const owners=all?.filter(e=>e.id===badgeOwner)??[],owner=owners.length===1?owners[0]:undefined,ownerData=object(owner?.customData),badgeIds=ownerData.visualBadgeIds;
  const primaryMatches=all?.filter(e=>e.id===ownerData.visualAssetElementId)??[],primary=primaryMatches.length===1?primaryMatches[0]:undefined;
  const badgeVerified=badgeOwner&&all?all.filter(e=>e.id===element.id).length===1&&Boolean(owner)&&Array.isArray(badgeIds)&&badgeIds.filter(v=>v===element.id).length===1&&primary?.type==='image'&&object(primary.customData).semanticAnchorId===owner?.id&&element.frameId===owner?.frameId:null;
  return { display_crop: displayCrop(element.crop), element_id: string(element.id), semantic_badge_anchor_id:badgeOwner,badge_meaning:badgeMeaning,badge_ownership_verified:badgeVerified,badge_scientific_meaning_verified:false, semantic_anchor_id: string(data.semanticAnchorId), mime_type: string(file.mimeType), file_id: fileId, ...source, ...hash, hash_matches_recorded: matchesRecordedHash(hash.actual_sha256, source.recorded_sha256), replacement_history: await replacementHistory(data.asset_replacements, lookup) };
}
/** Same whitelist and file checks as export, without serializing a whole scene. */
export function inspectAssetSource(element: unknown, fileInput: unknown) {
  const files = object(fileInput);
  return assetRecord(object(element), files, fileLookup(files));
}
export async function createSourceManifest(input: { scene: string; title: string; project_id?: string; figure_id?: string; based_on_revision?: number; unsaved_changes: boolean }) {
  const document = object(JSON.parse(input.scene)), files = object(document.files);
  const all=(Array.isArray(document.elements)?document.elements:[]).map(object).filter(e=>e.isDeleted!==true);
  const elements=all.filter(e=>e.type==="image");
  const lookup = fileLookup(files);
  const assets = await Promise.all(elements.map(element => assetRecord(element, files, lookup, all)));
  const credits = unique(assets.map(asset => `${asset.name} — ${asset.authors.length ? asset.authors.join(", ") : "author not recorded"}; ${asset.licenses.length ? asset.licenses.join(", ") : "license not recorded"}${asset.license_urls.length ? ` (${asset.license_urls.join(", ")})` : ""}; ${asset.source_urls.join(", ") || "source not recorded"}${asset.normalization.length || asset.modifications.length ? `; changes: ${[...asset.normalization, ...asset.modifications.map(change => change.kind ?? "unspecified")].join("; ")}` : ""}${asset.display_crop ? `; display: ${asset.display_crop.status === "recorded" ? "native source crop (source file unchanged)" : "invalid native crop metadata"}` : ""}`));
  return { schema: "autonomics.figure.asset-sources/v1", exported_at: new Date().toISOString(), title: input.title, project_id: input.project_id ?? null, figure_id: input.figure_id ?? null, based_on_revision: input.based_on_revision ?? null, unsaved_changes: input.unsaved_changes, scene_json_sha256: await sha256(new TextEncoder().encode(input.scene)), assets, credits, relation_sources: relationSources(document) };
}
export type SourceManifest = Awaited<ReturnType<typeof createSourceManifest>>;
export function embedSvgSources(svg: SVGSVGElement, manifest: SourceManifest) {
  const metadata = document.createElementNS("http://www.w3.org/2000/svg", "metadata"); metadata.id = "autonomics-asset-sources";
  metadata.textContent = JSON.stringify(manifest); svg.insertBefore(metadata, svg.firstChild);
}
// PNG international-text chunk: uncompressed UTF-8 keeps names and licence text intact.
const keyword = "autonomics.asset-sources";
function crc32(bytes: Uint8Array) {
  let crc = 0xffffffff;
  for (const byte of bytes) { crc ^= byte; for (let bit = 0; bit < 8; bit++) crc = (crc >>> 1) ^ (crc & 1 ? 0xedb88320 : 0); }
  return (crc ^ 0xffffffff) >>> 0;
}
export async function embedPngSources(blob: Blob, manifest: SourceManifest) {
  const bytes = new Uint8Array(await blob.arrayBuffer());
  if (bytes.length < 20 || [137, 80, 78, 71, 13, 10, 26, 10].some((v, i) => bytes[i] !== v)) throw new Error(i18n.t("canvas:sourceManifest.invalidPng"));
  const data = new TextEncoder().encode(`${keyword}\0\0\0\0\0${JSON.stringify(manifest)}`), type = new TextEncoder().encode("iTXt");
  const chunk = new Uint8Array(data.length + 12), view = new DataView(chunk.buffer); view.setUint32(0, data.length); chunk.set(type, 4); chunk.set(data, 8); view.setUint32(data.length + 8, crc32(chunk.subarray(4, data.length + 8)));
  const parts: Uint8Array[] = [bytes.slice(0, 8)]; let offset = 8, ended = false;
  while (offset + 12 <= bytes.length) {
    const length = new DataView(bytes.buffer, bytes.byteOffset + offset, 4).getUint32(0), end = offset + 12 + length;
    if (end > bytes.length) throw new Error(i18n.t("canvas:sourceManifest.invalidChunk"));
    const name = new TextDecoder().decode(bytes.subarray(offset + 4, offset + 8));
    if (name === "IEND") { parts.push(chunk, bytes.slice(offset, end)); ended = true; break; }
    const old = name === "iTXt" && length >= keyword.length + 1 && new TextDecoder().decode(bytes.subarray(offset + 8, offset + 8 + keyword.length + 1)) === `${keyword}\0`;
    if (!old) parts.push(bytes.slice(offset, end)); offset = end;
  }
  if (!ended) throw new Error(i18n.t("canvas:sourceManifest.missingIend"));
  return new Blob(parts.map(part => Uint8Array.from(part).buffer), { type: "image/png" });
}

/** Explicit scientific references only; never copy arbitrary customData into exports. */
type Row = Record<string,unknown>;
const row=(v:unknown):Row=>v&&typeof v==='object'&&!Array.isArray(v)?v as Row:{};
const id=(v:unknown)=>typeof v==='string'&&v.length>0&&v.length<=512?v:null;
function reference(value:unknown) {
  if(typeof value==='string'&&value.trim()&&!/^\s*data:/i.test(value))return {kind:'text',text:value.slice(0,2000),truncated:value.length>2000};
  if(!value||typeof value!=='object'||Array.isArray(value))return {kind:'unsupported'};
  const source=row(value),fields:Record<string,string>={};let omitted=false,truncated=false;
  for(const [key,v] of Object.entries(source)) {
    if(!['doi','pmid','pmcid','url','title','reference'].includes(key)||typeof v!=='string'||!v.trim()||/^\s*data:/i.test(v)){omitted=true;continue;}
    if(key==='url'){try{const url=new URL(v);if(!['http:','https:'].includes(url.protocol)||url.username||url.password){omitted=true;continue;}}catch{omitted=true;continue;}}
    fields[key]=v.slice(0,2000);if(v.length>2000)truncated=true;
  }
  return {kind:'structured',fields,omitted_fields:omitted,truncated};
}
export function relationSources(input:unknown) {
  const scene=row(input),elements=Array.isArray(scene.elements)?scene.elements:[];
  const relations=[];let total=0;
  for(const value of elements) {
    const e=row(value),data=row(e.customData);
    if(e.type!=='arrow'||e.isDeleted===true||data.annotationKind!=null||data.inset!=null)continue;
    total++;if(relations.length>=1000)continue;
    const evidence=data.evidence,entries=Array.isArray(evidence)?evidence:[];
    const actualHead=(v:unknown)=>v==null?null:typeof v==='string'&&v.length<=32?v:'unknown';
    relations.push({element_id:id(e.id),start_element_id:id(row(e.startBinding).elementId),end_element_id:id(row(e.endBinding).elementId),start_arrowhead:actualHead(e.startArrowhead),end_arrowhead:actualHead(e.endArrowhead),declared_direction:typeof data.direction==='string'&&['forward','bidirectional','undirected'].includes(data.direction)?data.direction:null,declared_inhibitory:typeof data.inhibitory==='boolean'?data.inhibitory:null,evidence_status:evidence==null?'not_recorded':Array.isArray(evidence)?'recorded':'unsupported',references:entries.slice(0,100).map(reference),reference_count:entries.length,references_truncated:entries.length>100});
  }
  return {schema:'autonomics.figure.relation-sources/v1',verification:'not_verified',relations,total,truncated:total>relations.length};
}
