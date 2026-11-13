// Source-backed second-component materialization; no scene is imported here.
import assert from 'node:assert/strict';
import {createHash} from 'node:crypto';
import {readFile,writeFile} from 'node:fs/promises';
import {join} from 'node:path';
import {currentCandidateLeaf} from './canvas-live-scientific.mjs';
import {checkLocalComponentLabelMove} from './canvas-materialized-components.mjs';
import {checkCandidateGeometryReviews} from './canvas-scene-geometry-evidence.mjs';
import {checkPortableSceneExport} from './canvas-live-source-review.mjs';
import {illustrationConnectorLabelOverlaps} from '../../src/features/canvas/canvas-connector-label-overlaps.ts';
const byId=(doc,id)=>doc.elements.find(e=>e.id===id);
export function statDimerTarget(plan){
 const dimers=plan.nodes.filter(n=>/STAT1(?!\d)/i.test(n.label)&&/GAF|二聚|dimer/i.test(n.label));
 const cytoplasmic=dimers.filter(n=>/胞质|细胞质|cyto/i.test(n.label)&&!/核内|核质|nuclear|nucleus/i.test(n.label));
 const candidates=cytoplasmic.length?cytoplasmic:dimers.filter(n=>!/核|nuclear|nucleus/i.test(n.label));
 assert.equal(candidates.length,1,'one explicit extranuclear STAT1 dimer is required; never guess another protein or nuclear state');
 return candidates[0];
}
export function checkSecondMaterialization(before,after,target,noteId){
 assert.equal(new Set(after.elements.map(e=>e.id)).size,after.elements.length,'native IDs remain unique');
 const oldImages=before.elements.filter(e=>e.type==='image'&&!e.isDeleted),images=after.elements.filter(e=>e.type==='image'&&!e.isDeleted);
 assert.equal(oldImages.length,1);assert.equal(images.length,2);
 const image=images.find(e=>e.customData?.semanticAnchorId===target.id);assert.ok(image);
 assert.notEqual(image.fileId,oldImages[0].fileId);assert.notEqual(image.customData.asset.id,oldImages[0].customData.asset.id);
 const asset=image.customData.asset;assert.match(asset.title??asset.name,/STAT/i);assert.match(asset.title??asset.name,/dimer|二聚/i);
 assert.ok(asset.creators?.length&&asset.licenses?.length);assert.equal(asset.source_url,`https://bioart.niaid.nih.gov/bioart/${asset.id}`);
 const file=after.files[image.fileId];assert.ok(file?.dataURL?.startsWith('data:image/png;base64,'));
 assert.equal(createHash('sha256').update(Buffer.from(file.dataURL.split(',')[1],'base64')).digest('hex'),image.customData.sha256);
 const anchor=byId(after,target.id),oldLabel=before.elements.find(e=>e.type==='text'&&e.containerId===target.id);assert.ok(oldLabel);
 const label=byId(after,oldLabel.id);assert.equal(anchor.customData.visualAssetElementId,image.id);assert.ok(anchor.customData.visualLabelIds.includes(label.id));assert.equal(label.containerId,null);
 for(const key of Object.keys(target.customData??{}))assert.deepEqual(anchor.customData[key],target.customData[key]);
 for(const key of ['originalText','fontSize','fontFamily','lineHeight'])assert.deepEqual(label[key],oldLabel[key]);
 assert.equal(label.text.replace(/\s/g,''),oldLabel.text.replace(/\s/g,''));
 const arrows=before.elements.filter(e=>e.type==='arrow'&&(e.startBinding?.elementId===target.id||e.endBinding?.elementId===target.id));
 const captions=before.elements.filter(e=>e.type==='text'&&arrows.some(a=>e.containerId===a.id));
 for(const old of arrows){const next=byId(after,old.id);assert.deepEqual(next.customData,old.customData);for(const key of ['startArrowhead','endArrowhead'])assert.deepEqual(next[key],old[key]);for(const key of ['startBinding','endBinding'])assert.equal(next[key].elementId,old[key].elementId);}
 for(const old of captions){const next=byId(after,old.id);for(const key of ['text','originalText','fontSize','fontFamily','lineHeight'])assert.deepEqual(next[key],old[key]);}
 const allowed=new Set([target.id,oldLabel.id,...arrows.map(e=>e.id),...captions.map(e=>e.id)]);
 for(const old of before.elements)if(!allowed.has(old.id))assert.deepEqual(byId(after,old.id),old,`second materialization preserves ${old.id}`);
 for(const [id,file]of Object.entries(before.files))assert.deepEqual(after.files[id],file);
 assert.deepEqual(Object.fromEntries(Object.entries(after).filter(([k])=>!['elements','files'].includes(k))),Object.fromEntries(Object.entries(before).filter(([k])=>!['elements','files'].includes(k))));
 const additions=after.elements.filter(e=>!byId(before,e.id));assert.equal(additions.length,2);assert.ok(additions.some(e=>e.id===image.id));
 const note=byId(after,noteId);assert.ok(note?.type==='text'&&!note.containerId&&!note.frameId&&note.fontSize===16);assert.equal(note.originalText,'STAT 素材为家族级示意，不验证 STAT1 特异结构或磷酸化位点。');assert.ok(note.y>=Math.max(...before.elements.filter(e=>!e.isDeleted).map(e=>e.y+e.height))+8);
 return {image,label};
}
export async function runScientificMultiArt({page,call,path,send,persist,directory,turns,until,planning}){
 const formal=await call('GET',path),revision=formal.metadata.revision,before=formal.scene.document;
 const semantic=statDimerTarget(planning.document),target=before.elements.find(e=>e.customData?.scientificId===semantic.id);assert.ok(target&&!target.customData.visualAssetElementId);
 const brief=await call('POST',`${path}/proposals`,{expected_revision:revision,summary:'Reviewed source-backed second component',operations:[]});
 const parent=await call('GET',`${path}/proposals/${brief.id}`),dialog=page.getByRole('dialog',{name:'Agent 修改预览',exact:true});
 const all=()=>call('GET',`${path}/proposals?limit=100`);
 const show=async candidate=>{
  await page.getByRole('tab',{name:'Live figure',exact:true}).click();await page.getByRole('button',{name:'查看 Agent 提案',exact:true}).click();
  const items=(await call('GET',`${path}/proposal-index?limit=100`)).items,matching=items.filter(p=>p.summary===candidate.summary&&p.base_revision===revision&&Boolean(p.parent_proposal_id)===Boolean(candidate.parent_proposal_id));
  const index=matching.findIndex(p=>p.id===candidate.id);assert.ok(index>=0);
  await dialog.getByRole('button',{name:`${candidate.summary} · v${revision}${candidate.parent_proposal_id?' · 修订':''}`,exact:true}).nth(index).click();await dialog.getByRole('region',{name:'候选画布'}).locator('.excalidraw').waitFor();
 };
 const feedback=async(candidate,text)=>{await show(candidate);const detail=dialog.locator('details[aria-label="候选修订反馈"]');await detail.locator('summary').click();await detail.getByRole('textbox',{name:'候选修订要求',exact:true}).fill(text);await detail.getByRole('button',{name:'准备候选修订草稿',exact:true}).click();await page.getByPlaceholder('输入消息…').waitFor();};
 const start=turns.length,noteId='stat-family-art-note';
 await feedback(parent,`在这张已确认来源且已有受体素材的图上，为 ${target.id}（${semantic.label}）再选一份合适的 NIH BioArt 素材。先检索 STAT activated dimer、transcription factor 等候选，再实际 preview 选定图后 replace 当前节点；不把 STAT1 无命中当作没有通用示意图。保留 STAT1/GAF/胞质与修饰状态的全部原标签和来源，不声称图标验证 STAT1 特异结构。保留已有受体图片、标签、相关箭头及所有其他节点/说明/文件不变；可只调整新素材的自有标签及必要的关联连线几何以避免重叠，方向和依据不变。检查 PNG 的实际范围，必要时使用 visualization_fit。新增一个独立可编辑说明 ${noteId}，内容为“STAT 素材为家族级示意，不验证 STAT1 特异结构或磷酸化位点。”，字号16，位于现有图注下面至少8单位；用 fit_width:true，宽度上限不超过核心图宽。不要重新生成草图或另起根候选，所有修改顺序延续当前叶候选 ID/哈希。读取最终候选及PNG，并核对完整场景几何指标，留待审阅，不采用。没有合适素材时明确报告，不使用不相关蛋白图。`);
 await send(start+1);const candidates=await all(),child=currentCandidateLeaf(candidates,revision);await persist('multi-art-first-child.json',child);
 const ancestor=new Set();let cursor=child;while(cursor.id!==parent.id){assert.ok(!ancestor.has(cursor.id));ancestor.add(cursor.id);cursor=candidates.find(p=>p.id===cursor.parent_proposal_id);assert.ok(cursor,'same intended source-backed candidate lineage');}
 const calls=turns[start].calls.filter(c=>!c.is_error),replacements=calls.filter(c=>c.name==='bioart'&&c.input.action==='replace');assert.ok(replacements.length);
 for(const replacement of replacements){assert.equal(replacement.input.element_id,target.id);const base=candidates.find(p=>p.id===replacement.input.proposal_id);assert.ok(base&&(base.id===parent.id||ancestor.has(base.id)));assert.equal(replacement.input.document_sha256,base.document_sha256);const earlier=calls.slice(0,calls.indexOf(replacement));assert.ok(earlier.some(c=>c.name==='bioart'&&c.input.action==='search'));assert.ok(earlier.some(c=>c.name==='bioart'&&c.input.action==='preview'&&c.input.asset_id===replacement.input.asset_id));}
 assert.deepEqual(child.review_markups,parent.review_markups);
 const {image,label}=checkSecondMaterialization(before,child.document,target,noteId);
 const replacement=replacements.at(-1),sourceCandidate=candidates.find(p=>p.parent_proposal_id===replacement.input.proposal_id);assert.ok(sourceCandidate);
 assert.deepEqual(image.customData.asset,sourceCandidate.document.elements.find(e=>e.type==='image'&&e.customData?.semanticAnchorId===target.id).customData.asset,'catalogue attribution is retained from the actual immutable replacement result');
 assert.equal(image.customData.asset.id,replacement.input.asset_id);assert.deepEqual(await call('GET',path),formal);
 const render=async(candidate,name)=>{await call('POST',`${path}/proposals/${candidate.id}/preview-request`,{expected_revision:revision});let png;await until(async()=>{png=await call('GET',`${path}/proposals/${candidate.id}/preview?expected_revision=${revision}`);assert.notEqual(png.render_status,'failed');return png.render_status==='ready';},'multi-component native PNG');assert.equal(png.document_sha256,candidate.document_sha256);await writeFile(join(directory,name),Buffer.from(png.dataURL.split(',')[1],'base64'));};
 await render(child,'multi-art-first.png');
 await feedback(child,`只把 ${label.id} 这一段新 STAT 素材自有标签向下移动8个画布单位（dx:0, dy:8），使用 visualization_move_text；不要扩展整个组，不移动任何图片、语义锚点、连线、其他标签或说明，不改文字/换行/字号/来源。沿当前候选继续，读取新候选与PNG并核对完整场景指标，不采用。`);
 await send(start+2);const updated=await all(),finalChild=currentCandidateLeaf(updated,revision);let base=finalChild;const seen=new Set();while(base.id!==child.id){assert.ok(!seen.has(base.id));seen.add(base.id);base=updated.find(p=>p.id===base.parent_proposal_id);assert.ok(base);}
 assert.ok(turns[start+1].calls.some(c=>!c.is_error&&c.name==='visualization_move_text'&&c.input.element_id===label.id&&c.input.dx===0&&c.input.dy===8));
 assert.deepEqual(finalChild.review_markups,child.review_markups);checkLocalComponentLabelMove(child.document,finalChild.document,label.id,8);assert.deepEqual(await call('GET',path),formal);
 const history=JSON.parse(await readFile(join(directory,'session-history.json'),'utf8'));await persist('multi-art-geometry-review.json',checkCandidateGeometryReviews(history,finalChild,{projectId:formal.metadata.project_id}));
 const clearance=illustrationConnectorLabelOverlaps(finalChild.document);assert.ok(clearance.checked>0);assert.equal(clearance.skipped,0);assert.equal(clearance.omitted,0);assert.equal(clearance.count,0);await persist('multi-art-label-clearance.json',clearance);
 await render(finalChild,'multi-art-final.png');await show(finalChild);await page.screenshot({path:join(directory,'multi-art-review.png'),animations:'disabled'});await dialog.getByRole('button',{name:'采用提案',exact:true}).click();await dialog.waitFor({state:'hidden'});
 const adopted=await call('GET',path);assert.equal(adopted.metadata.revision,revision+1);assert.deepEqual(adopted.scene.document,finalChild.document);assert.deepEqual(await call('GET',`${path}/planning`),planning);
 const download=page.waitForEvent('download');await page.getByRole('button',{name:'导出 源文件',exact:true}).click();const exported=JSON.parse(await readFile(await(await download).path(),'utf8'));checkPortableSceneExport(exported,finalChild.document);await persist('multi-art-source.json',exported);await persist('multi-art-final.json',adopted);
 return {target_id:target.id,scientific_id:semantic.id,asset_id:image.customData.asset.id,asset_author:image.customData.asset.creators,asset_license:image.customData.asset.licenses,distinct_materials_verified:true,local_label_preservation_verified:true,full_scene_geometry_review_verified:true,scope:'second distinct catalogue illustration on the same source-backed generated figure; STAT family artwork is not STAT1 structural or scientific validation'};
}
