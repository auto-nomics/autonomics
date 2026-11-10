/** Bounded, explicit canvas context shared by the workbench channel and figure handoff. */
type Row = Record<string, unknown>;
const row = (value: unknown): Row => value && typeof value === "object" && !Array.isArray(value) ? value as Row : {};
const id = (value: unknown): string | null => typeof value === "string" && value.length > 0 && value.length <= 512 ? value : null;
const finite = (value: unknown) => typeof value === "number" && Number.isFinite(value) ? value : null;
function rectangle(value: unknown, keys: string[]) {
  const input = row(value);
  return Object.fromEntries(keys.map(key => [key, finite(input[key])]));
}
function insetReference(value: unknown) {
  const input = row(value);
  if (input.kind !== "image_inset_snapshot") return null;
  return { kind: input.kind, anchor_id: id(input.anchor_id), image_id: id(input.image_id), region_id: id(input.region_id), callout_id: id(input.callout_id), source_element_id: id(input.source_element_id), source_revision: typeof input.source_revision === "number" && Number.isSafeInteger(input.source_revision) && input.source_revision > 0 ? input.source_revision : null, region: rectangle(input.region, ["x", "y", "width", "height"]) };
}
export function canvasSelectionContext(input: readonly unknown[], selected: Readonly<Record<string, unknown>>) {
  const elements = input.map(row).filter(element => element.isDeleted !== true);
  const index = new Map(elements.filter(element => id(element.id)).map(element => [String(element.id), element]));
  const references = new Map<string, { reference: NonNullable<ReturnType<typeof insetReference>>; verified: boolean }>();
  const titles = new Map<string, string>();
  for (const element of elements) {
    const data = row(element.customData), reference = insetReference(data.inset);
    if (!reference?.anchor_id || reference.anchor_id !== element.id) continue;
    const image = index.get(reference.image_id ?? ""), region = index.get(reference.region_id ?? ""), callout = index.get(reference.callout_id ?? "");
    const identities = [reference.anchor_id, reference.image_id, reference.region_id, reference.callout_id];
    const validRegion = Object.values(reference.region).every(value => value !== null) && (reference.region.x ?? -1) >= 0 && (reference.region.y ?? -1) >= 0 && (reference.region.width ?? 0) > 0 && (reference.region.height ?? 0) > 0 && (reference.region.x ?? 0) + (reference.region.width ?? 0) <= 1.000000001 && (reference.region.y ?? 0) + (reference.region.height ?? 0) <= 1.000000001;
    const verified = validRegion && Boolean(reference.source_element_id) && reference.source_revision !== null && identities.every(Boolean) && new Set(identities).size === 4 && !identities.includes(reference.source_element_id)
      && element.type === "rectangle" && image?.type === "image" && region?.type === "rectangle" && callout?.type === "arrow"
      && data.visualAssetElementId === reference.image_id && row(image?.customData).semanticAnchorId === reference.anchor_id
      && row(region?.customData).annotationKind === "inset_region" && row(callout?.customData).annotationKind === "inset_callout"
      && [image, region, callout].every(member => JSON.stringify(insetReference(row(member?.customData).inset)) === JSON.stringify(reference));
    references.set(reference.anchor_id, { reference, verified });
    if (verified && Array.isArray(data.visualLabelIds)) for (const label of data.visualLabelIds) {
      const title = index.get(String(label)), groups = title?.groupIds;
      if (id(label) && title?.type === "text" && typeof data.visualAssetGroupId === "string" && Array.isArray(groups) && groups.includes(data.visualAssetGroupId)) titles.set(String(label), reference.anchor_id);
    }
  }
  const chosen = elements.filter(element => typeof element.id === "string" && selected[element.id] === true);
  const objects: Row[] = []; let chars = 2;
  for (const element of chosen) {
    if (objects.length === 100) break;
    if (!id(element.id)) continue;
    const data = row(element.customData), ownReference = insetReference(data.inset);
    const anchor = ownReference?.anchor_id ?? titles.get(String(element.id));
    const resolved = anchor ? references.get(anchor) : undefined;
    const reference = ownReference ?? resolved?.reference;
    const memberRole = reference && resolved?.verified && JSON.stringify(reference) === JSON.stringify(resolved.reference)
      ? element.id === reference.anchor_id ? "inset_panel" : element.id === reference.image_id ? "inset_image" : element.id === reference.region_id ? "inset_region" : element.id === reference.callout_id ? "inset_callout" : titles.get(String(element.id)) === reference.anchor_id ? "inset_title" : null : null;
    const object: Row = { id: element.id, type: id(element.type), ...rectangle(element, ["x", "y", "width", "height", "angle"]), role: memberRole ?? (reference ? "unverified_inset_reference" : "canvas_object") };
    if (typeof element.text === "string") { object.text = element.text.slice(0, 1000); object.text_truncated = element.text.length > 1000; }
    if (id(element.frameId)) object.frame_id = element.frameId;
    if (Array.isArray(element.groupIds) && element.groupIds.length) {
      object.group_ids = element.groupIds.filter(value => id(value)).slice(0, 32);
      object.group_ids_truncated = (object.group_ids as unknown[]).length < element.groupIds.length;
    }
    if (element.type === "image") {
      object.file_id = id(element.fileId);
      object.scale = Array.isArray(element.scale) && element.scale.length === 2 && element.scale.every(value => value === 1 || value === -1) ? element.scale : null;
      object.crop = element.crop ? rectangle(element.crop, ["x", "y", "width", "height", "naturalWidth", "naturalHeight"]) : null;
      if (id(data.semanticAnchorId)) { object.semantic_anchor_id = data.semanticAnchorId; object.semantic_anchor_verified = row(index.get(String(data.semanticAnchorId))?.customData).visualAssetElementId === element.id; }
    }
    if (reference) object.inset = { ...reference, identity_verified: Boolean(memberRole), source_available: index.get(reference.source_element_id ?? "")?.type === "image" };
    if (element.type === "arrow") { object.start_binding_id = id(row(element.startBinding).elementId); object.end_binding_id = id(row(element.endBinding).elementId); }
    const size = JSON.stringify(object).length + 1;
    if (chars + size > 16000) continue;
    chars += size; objects.push(object);
  }
  return { objects, ids: objects.map(element => String(element.id)).sort(), total: chosen.length, truncated: objects.length < chosen.length };
}
