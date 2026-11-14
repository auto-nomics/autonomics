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
    const fitStatus=illustrationFitStatus(elements,String(element.id));if(fitStatus)object.illustration_fit=fitStatus;
    if (typeof element.text === "string") { object.text = element.text.slice(0, 1000); object.text_truncated = element.text.length > 1000; }
    if (element.type === "text") {
      object.font_family = finite(element.fontFamily); object.font_size = finite(element.fontSize); object.line_height = finite(element.lineHeight);
      object.container_id = id(element.containerId);
      object.auto_resize = typeof element.autoResize === "boolean" ? element.autoResize : null;
      object.text_align = typeof element.textAlign === "string" && ["left", "center", "right"].includes(element.textAlign) ? element.textAlign : null;
      object.vertical_align = typeof element.verticalAlign === "string" && ["top", "middle", "bottom"].includes(element.verticalAlign) ? element.verticalAlign : null;
    }
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
    if(Array.isArray(data.visualBadgeIds)){object.badge_ids=[...new Set(data.visualBadgeIds.filter(value=>id(value)))].slice(0,32);object.badge_ids_truncated=(object.badge_ids as unknown[]).length<data.visualBadgeIds.length;}
    if(element.type==='image'&&id(data.semanticBadgeAnchorId)){
      const ownerMatches=elements.filter(e=>e.id===data.semanticBadgeAnchorId),owner=ownerMatches.length===1?ownerMatches[0]:undefined,ownerData=row(owner?.customData),badgeIds=ownerData.visualBadgeIds;
      const primaryMatches=elements.filter(e=>e.id===ownerData.visualAssetElementId),primary=primaryMatches.length===1?primaryMatches[0]:undefined;
      const verified=elements.filter(e=>e.id===element.id).length===1&&Boolean(owner)&&Array.isArray(badgeIds)&&badgeIds.filter(v=>v===element.id).length===1&&primary?.type==='image'&&row(primary.customData).semanticAnchorId===owner?.id&&element.frameId===owner?.frameId;
      const group=id(data.visualBadgeGroupId);object.semantic_badge_anchor_id=data.semanticBadgeAnchorId;object.badge_meaning=typeof data.badgeMeaning==='string'&&!/^\s*data:/i.test(data.badgeMeaning)?data.badgeMeaning.slice(0,512):null;object.semantic_badge_anchor_verified=verified;object.badge_group_verified=verified&&Boolean(group)&&group===ownerData.visualAssetGroupId&&[element,owner,primary].every(e=>Array.isArray(e?.groupIds)&&e.groupIds.includes(group));object.badge_scientific_meaning_verified=false;
    }
    if (reference) object.inset = { ...reference, identity_verified: Boolean(memberRole), source_available: index.get(reference.source_element_id ?? "")?.type === "image" };
    if (element.type === "arrow") {
      object.start_binding_id = id(row(element.startBinding).elementId); object.end_binding_id = id(row(element.endBinding).elementId);
      object.start_arrowhead = id(element.startArrowhead); object.end_arrowhead = id(element.endArrowhead);
      const direction = row(element.customData).direction;
      if (typeof row(element.customData).inhibitory === "boolean") object.declared_inhibitory = row(element.customData).inhibitory;
      if (direction === "forward" || direction === "bidirectional" || direction === "undirected") object.declared_direction = direction;
    }
    const size = JSON.stringify(object).length + 1;
    if (chars + size > 16000) continue;
    chars += size; objects.push(object);
  }
  return { objects, ids: objects.map(element => String(element.id)).sort(), total: chosen.length, truncated: objects.length < chosen.length };
}

/** Offer a scoped read only for a complete, representable current selection. */
export function canvasReadInput(projectId: string, figureId: string, revision: number, selection: ReturnType<typeof canvasSelectionContext>) {
  const input = { action: "read", project_id: projectId, figure_id: figureId, expected_revision: revision };
  const ids = selection.objects.map(object => object.id);
  if (selection.truncated || selection.total < 1 || selection.total > 20 || ids.length !== selection.total || new Set(ids).size !== ids.length || ids.some(id => typeof id !== "string" || !id.trim() || new TextEncoder().encode(id).length > 512)) return input;
  return { ...input, element_ids: ids as string[] };
}

type FitRow=Record<string,unknown>;
const fitRow=(v:unknown):FitRow=>v&&typeof v==='object'&&!Array.isArray(v)?v as FitRow:{};
const identity=(v:unknown):v is string=>typeof v==='string'&&v.length>0&&v.length<=512;
const fitFinite=(v:unknown):v is number=>typeof v==='number'&&Number.isFinite(v);
function fitSnapshotStatus(fit:FitRow,image:FitRow){
 const bounds=fit.imageBounds,normalized=fit.normalizedBounds,transform=fitRow(fit.imageTransform);
 if(!identity(fit.fileId)||!Array.isArray(bounds)||bounds.length!==4||!bounds.every(fitFinite)||bounds[2]<=0||bounds[3]<=0||!Array.isArray(normalized)||normalized.length!==4||!normalized.every(fitFinite)||normalized[0]<0||normalized[1]<0||normalized[2]<=0||normalized[3]<=0||normalized[0]+normalized[2]>1.000001||normalized[1]+normalized[3]>1.000001||!fitFinite(transform.angle)||!Array.isArray(transform.scale)||transform.scale.length!==2||!transform.scale.every(v=>v===1||v===-1)||!Object.hasOwn(transform,'crop'))return 'invalid_record' as const;
 if(fit.fileId!==image.fileId)return 'stale_file' as const;
 const current=[image.x,image.y,image.width,image.height];
 const cropKeys=['x','y','width','height','naturalWidth','naturalHeight'];
 const sameCrop=transform.crop===null?image.crop==null:image.crop!=null&&cropKeys.every(k=>fitFinite(fitRow(transform.crop)[k])&&fitRow(transform.crop)[k]===fitRow(image.crop)[k]);
 if(!current.every(fitFinite)||current.some((v,i)=>Math.abs(v-bounds[i])>1e-6)||transform.angle!==(image.angle??0)||!Array.isArray(image.scale)||!transform.scale.every((v,i)=>v===(image.scale as unknown[])[i])||!sameCrop)return 'stale_geometry' as const;
 return 'current_snapshot' as const;
}
/** Snapshot freshness only; no paint, collision, or scientific correctness inference. */
export function illustrationFitStatus(input:readonly unknown[],focusedId:string){
 const elements=input.map(fitRow).filter(e=>e.isDeleted!==true),matches=elements.filter(e=>e.id===focusedId);
 if(matches.length!==1)return null;
 const selected=matches[0],data=fitRow(selected.customData),anchorId=selected.type==='image'?data.semanticAnchorId:selected.id;
 if(!identity(anchorId))return null;
 const anchors=elements.filter(e=>e.id===anchorId),anchor=anchors.length===1?anchors[0]:undefined,owner=fitRow(anchor?.customData),imageId=owner.visualAssetElementId;
 if(!identity(imageId))return null;
 const images=elements.filter(e=>e.id===imageId),image=images.length===1?images[0]:undefined;
 if(!anchor||image?.type!=='image'||fitRow(image.customData).semanticAnchorId!==anchorId||(selected.type==='image'&&selected.id!==imageId))return null;
 const fit=fitRow(owner.visualAnchorFit),base={anchor_id:anchorId,image_id:imageId,recorded_file_id:identity(fit.fileId)?fit.fileId:null,current_file_id:identity(image.fileId)?image.fileId:null,scientific_validity_verified:false};
 if(owner.visualAnchorFit==null)return {...base,status:'not_recorded' as const};
 const primaryStatus=fitSnapshotStatus(fit,image);
 if(primaryStatus!=='current_snapshot')return {...base,status:primaryStatus};
 if(fit.scope==='illustration_group'){
  const badges=owner.visualBadgeIds??[],members=fit.members;
  if(!Array.isArray(badges)||!badges.every(identity)||new Set(badges).size!==badges.length||!Array.isArray(members)||members.length!==badges.length+1||members.length>33)return {...base,status:'invalid_record' as const};
  if(elements.filter(e=>e.type==='image'&&(fitRow(e.customData).semanticAnchorId===anchorId||fitRow(e.customData).semanticBadgeAnchorId===anchorId)).length!==members.length)return {...base,status:'invalid_record' as const};
  const expected=new Set([imageId,...badges]),seen=new Set<string>();
  for(const raw of members){const member=fitRow(raw),id=member.elementId;if(!identity(id)||!expected.has(id)||seen.has(id))return {...base,status:'invalid_record' as const};seen.add(id);const matches=elements.filter(e=>e.id===id),image=matches.length===1?matches[0]:undefined,data=fitRow(image?.customData);
   if(!image||image.type!=='image'||image.frameId!==anchor.frameId||!Array.isArray(image.groupIds)||!image.groupIds.includes(owner.visualAssetGroupId)||(id===imageId?data.semanticAnchorId!==anchorId:data.semanticBadgeAnchorId!==anchorId||data.visualBadgeGroupId!==owner.visualAssetGroupId))return {...base,status:'invalid_record' as const};
   const status=fitSnapshotStatus(member,image);if(status!=='current_snapshot')return {...base,status};
  }
  const b=fit.anchorBounds,current=[anchor.x,anchor.y,anchor.width,anchor.height];if(!Array.isArray(b)||b.length!==4||!b.every(fitFinite)||b[2]<=0||b[3]<=0)return {...base,status:'invalid_record' as const};if(!current.every(fitFinite)||current.some((v,i)=>Math.abs(v-b[i])>1e-6))return {...base,status:'stale_geometry' as const};
 }
 return {...base,status:'current_snapshot' as const};
}

/** Bounded checkpoint inventory; snapshot counts never establish scientific completion. */
export function canvasMaterializationProgress(input:readonly unknown[]){
 const elements=input.map(row).filter(e=>e.isDeleted!==true),anchors=elements.filter(e=>id(e.id)&&id(row(e.customData).scientificId));
 const components=anchors.slice(0,200).map(anchor=>{
  const data=row(anchor.customData),images=elements.filter(e=>e.type==='image'&&row(e.customData).semanticAnchorId===anchor.id),matches=elements.filter(e=>e.id===data.visualAssetElementId),image=matches.length===1?matches[0]:undefined;
  const owned=Array.isArray(data.visualLabelIds)?data.visualLabelIds:[];
  const labels=elements.filter(e=>e.type==='text'&&(e.containerId===anchor.id||owned.includes(e.id))).map(e=>typeof e.originalText==='string'?e.originalText:typeof e.text==='string'?e.text:'').filter(Boolean);
  const ambiguous=anchors.filter(e=>e.id===anchor.id).length!==1||images.length>1||(images.length===1&&!id(data.visualAssetElementId))||(data.visualAssetElementId!=null&&(image?.type!=='image'||images[0]?.id!==image.id));
  const group=data.visualAssetGroupId,nativeLabel=elements.some(e=>e.type==='text'&&e.frameId===anchor.frameId&&e.containerId===anchor.id&&Array.isArray(anchor.boundElements)&&anchor.boundElements.some(b=>row(b).id===e.id&&row(b).type==='text'));
  const detachedLabels=owned.length>0&&new Set(owned).size===owned.length&&owned.every(id=>elements.some(e=>e.id===id&&e.type==='text'&&e.frameId===anchor.frameId&&e.containerId==null&&typeof group==='string'&&Array.isArray(e.groupIds)&&e.groupIds.includes(group)));
  const fit=illustrationFitStatus(elements,String(anchor.id));
  return {has_verified_label_ownership:nativeLabel||detachedLabels,anchor_id:String(anchor.id),scientific_id:id(data.scientificId),label:labels.join(' · ').slice(0,160),image_id:image?.type==='image'?id(image.id):null,status:ambiguous?'ambiguous':images.length===0?'missing':fit?.status==='current_snapshot'?'snapshot_current':'needs_fit'};
 });
 const unassigned=elements.filter(e=>e.type==='image').filter(image=>{
  const data=row(image.customData),primary=elements.filter(e=>e.id===data.semanticAnchorId),badge=elements.filter(e=>e.id===data.semanticBadgeAnchorId);
  const primaryValid=primary.length===1&&row(primary[0].customData).visualAssetElementId===image.id;
  const badgeIds=badge.length===1?row(badge[0].customData).visualBadgeIds:null;
  const badgeValid=Array.isArray(badgeIds)&&badgeIds.includes(image.id);
  return !primaryValid&&!badgeValid;
 });
 const unassignedImages={total:unassigned.length,represented:Math.min(unassigned.length,40),truncated:unassigned.length>40,items:unassigned.slice(0,40).map(e=>({image_id:id(e.id),declared_primary_owner:id(row(e.customData).semanticAnchorId),declared_badge_owner:id(row(e.customData).semanticBadgeAnchorId)}))};
 const labelIssues=components.filter(c=>c.image_id&&!c.has_verified_label_ownership);
 return {label_ownership_issues:{total:labelIssues.length,items:labelIssues.slice(0,40).map(c=>c.anchor_id),truncated:labelIssues.length>40||anchors.length>components.length},scope:'scientific_anchor_ownership_and_fit_snapshot_only',unassigned_images:unassignedImages,total:anchors.length,represented:components.length,truncated:anchors.length>components.length,counts:{missing:components.filter(c=>c.status==='missing').length,needs_fit:components.filter(c=>c.status==='needs_fit').length,ambiguous:components.filter(c=>c.status==='ambiguous').length,snapshot_current:components.filter(c=>c.status==='snapshot_current').length},components,scientific_validity_verified:false};
}
export function materializationContinuationRequest(progress:ReturnType<typeof canvasMaterializationProgress>){
 if(progress.truncated||progress.counts.ambiguous)return null;
 const pending=progress.components.filter(c=>c.status!=='snapshot_current');
 if(!pending.length)return null;
 const ordered=[...pending.filter(c=>c.status==='needs_fit'),...pending.filter(c=>c.status==='missing')],batch=ordered.slice(0,2);
 return `从当前已保存画布分步推进整图素材化，不从头重建。先读取当前画布、文件与PNG核对实际状态。本轮先推进这${batch.length}个组件：${batch.map(c=>`${c.anchor_id}（${c.label||c.scientific_id}；${c.status==='needs_fit'?'已有主图，需核对当前文件并贴合':'缺少主图，需检索、预览和选择'}）`).join('、')}。保留其他组件、全部科学文字与字号、素材来源、关系方向和历史计划；已有素材不重新检索替换。${progress.unassigned_images.total?`另有${progress.unassigned_images.total}张图片尚未核实组件归属（${progress.unassigned_images.items.map(e=>e.image_id).join("、")}${progress.unassigned_images.truncated?"，列表有省略":""}）；先读取并核对是否为本轮已插入素材或独立背景，避免重复插入，不自动推断或绑定其科学身份。`:""}直接更新当前画布并使用返回的新版本继续，读取结果PNG，报告本轮改动、检查范围及尚未完成的${Math.max(0,pending.length-batch.length)}个组件。整图目标仍是覆盖全部${progress.total}个组件，本轮分步推进不代表整图完成。`;
}
