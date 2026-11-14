import {readCandidatePreparedPrompt} from './canvas-prepared-context.mjs';
// Actual model receives explicit reviewer feedback on a supplied saved figure.
// This does not test autonomous discovery or autonomous scientific peer review.
import assert from 'node:assert/strict';
import {readFile,writeFile} from 'node:fs/promises';
import {createHash} from 'node:crypto';
import {join} from 'node:path';
import {illustrationConnectorLabelOverlaps} from '../../src/features/canvas/canvas-connector-label-overlaps.ts';
import {textOverlapChanges} from '../../src/features/canvas/canvas-text-overlaps.ts';
import {currentCandidateLeaf} from './canvas-live-scientific.mjs';
import {sourceRecords} from './canvas-live-evidence.mjs';
import {checkPortableSceneExport} from './canvas-live-source-review.mjs';
const text=e=>e.originalText??e.text;
const omit=(value,keys)=>Object.fromEntries(Object.entries(value).filter(([key])=>!keys.includes(key)));
const bookkeeping=['version','versionNonce','updated'];
export function reviewedRepairTargets(document){
 const labels=document.elements.filter(e=>e.type==='text'&&!e.isDeleted&&/ERK1\/2/.test(text(e)??'')&&/p-2T MAPK/.test(text(e)??''));
 assert.equal(labels.length,1,'one explicitly reviewed ERK shorthand label');
 const overlap=illustrationConnectorLabelOverlaps(document);assert.equal(overlap.count,1,'one observed owned-label connector crossing');
 const crossing=overlap.pairs[0],label=document.elements.find(e=>e.id===crossing.label_id);
 assert.ok(label&&!label.containerId,'illustration label is independent native text');
 return {erk_id:labels[0].id,expected_erk_text:text(labels[0]).replace('p-2T MAPK','活化态'),receptor_label_id:label.id,crossing};
}
export function checkReviewedRepair(previous,next,targets){
 assert.deepEqual(omit(next,['elements']),omit(previous,['elements']),'files, sources and scene settings survive');
 assert.equal(next.elements.length,previous.elements.length);
 for(const old of previous.elements){
  const actual=next.elements.find(e=>e.id===old.id);assert.ok(actual);
  if(old.id===targets.erk_id){
   assert.equal(text(actual),targets.expected_erk_text);
   assert.equal(actual.text.replace(/\s/g,''),targets.expected_erk_text.replace(/\s/g,''));
   assert.deepEqual(omit(actual,[...bookkeeping,'text','originalText','x','y','width','height','autoResize']),omit(old,[...bookkeeping,'text','originalText','x','y','width','height','autoResize']),'ERK edit only reflows in the existing container');
  }else if(old.id===targets.receptor_label_id){
   assert.equal(text(actual),text(old));assert.equal(actual.text.replace(/\s/g,''),text(old).replace(/\s/g,''));
   assert.deepEqual(omit(actual,[...bookkeeping,'x','y','width','height','text','autoResize']),omit(old,[...bookkeeping,'x','y','width','height','text','autoResize']),'illustration label geometry only');
   assert.ok([actual.x,actual.y,actual.width,actual.height].every(Number.isFinite)&&actual.width>0&&actual.height>0);
  }else {assert.deepEqual(actual,old,`unrelated ${old.id} unchanged`);continue;}
  assert.ok([actual.x,actual.y,actual.width,actual.height].every(Number.isFinite)&&actual.width>0&&actual.height>0);
  const frame=previous.elements.find(e=>e.id===old.frameId);
  if(frame)assert.ok(actual.x>=frame.x&&actual.y>=frame.y&&actual.x+actual.width<=frame.x+frame.width&&actual.y+actual.height<=frame.y+frame.height,'edited labels remain inside their original panel');
  const container=previous.elements.find(e=>e.id===old.containerId);
  if(container)assert.ok(actual.x>=container.x&&actual.y>=container.y&&actual.x+actual.width<=container.x+container.width&&actual.y+actual.height<=container.y+container.height,'bound text remains in its original container');
 }
 assert.equal(illustrationConnectorLabelOverlaps(next).count,0,'reviewed connector crossing removed');
 assert.equal(textOverlapChanges(previous,next).counts.introduced,0,'repair introduces no text rectangle overlap');
}
export async function runLiveReviewedRepair({page,call,path,send,persist,directory,turns,until,source}){
 const bytes=await readFile(source),input=JSON.parse(bytes),document=input.scene?.document??input.document??input;
 const targets=reviewedRepairTargets(document),head=await call('GET',path);
 await call('PUT',path,{expected_revision:head.metadata.revision,title:'Live figure',document});
 page.once('dialog',d=>d.accept());await page.getByRole('button',{name:'重新加载服务端版本',exact:true}).click();await page.locator('[data-canvas-surface] .excalidraw:visible').waitFor();
 const formal=await call('GET',path),revision=formal.metadata.revision;assert.deepEqual(formal.scene.document,document);
 const parent=await call('POST',`${path}/proposals`,{expected_revision:revision,summary:'审阅 ERK 来源冲突及受体标签穿线',operations:[]});
 await persist('reviewed-repair-seed.json',{source_sha256:createHash('sha256').update(bytes).digest('hex'),targets,formal,parent});
 await page.getByRole('button',{name:'查看 Agent 提案',exact:true}).click();const dialog=page.getByRole('dialog',{name:'Agent 修改预览',exact:true});
 await dialog.getByRole('button',{name:`${parent.summary} · v${revision}`,exact:true}).click();await dialog.getByRole('region',{name:'候选画布'}).locator('.excalidraw').waitFor();
 const request=[
  '这是对保存图稿的两项明确审阅意见，请定点修订，不重建图稿，也不要修改其他元数据。',
  `1. 重新读取 Reactome R-HSA-5672973：输出复合物名称含 p-2T MAPK，而正文说明活化环 threonine 和 tyrosine 磷酸化。核对这个来源字段冲突；仅将 ${targets.erk_id} 的完整文字改为「${targets.expected_erk_text}」，用 set_wrapped_text 在原绑定容器中重排。不要把矛盾简写继续作为残基化学结论，也不要新增残基编号或删引用。`,
  `2. ${targets.crossing.arrow_id} 穿过所属插图的标签 ${targets.receptor_label_id}。只调整这份独立标签的 x/y 和换行宽度，使其避开连线及其他文字，可放在插图旁边；完整原文、字号、字体、来源不变，仍在原分面内。不要移动图片、锚点、连线或其他标签。`,
  '先读取父候选与 PNG，逐步生成单一连续子候选链；读取最终候选 PNG 核对后留待审阅，不采用。除了上述两份文字允许的编辑，所有对象、文件、证据、关系、元数据及历史保持原样。'
 ].join('\n');
 const feedback=dialog.locator('details[aria-label="候选修订反馈"]');await feedback.locator('summary').click();
 await feedback.getByRole('textbox',{name:'候选修订要求',exact:true}).fill(request);await feedback.getByRole('button',{name:'准备候选修订草稿',exact:true}).click();
 let draft;await until(async()=>{draft=await readCandidatePreparedPrompt(page);return draft.includes(parent.id)&&draft.includes(parent.document_sha256);},'reviewed repair draft carries the current candidate identity');
 const structured=JSON.parse(draft.slice(draft.lastIndexOf('\n\n{')+2));assert.ok(structured.candidate_connector_label_check.pairs.some(pair=>pair.arrow_id===targets.crossing.arrow_id&&pair.label_id===targets.receptor_label_id));
 await persist('reviewed-repair-feedback.json',{request,draft});await send(1);
 const all=await call('GET',`${path}/proposals`);await persist('reviewed-repair-candidates.json',all);const child=currentCandidateLeaf(all,revision);assert.notEqual(child.id,parent.id);
 let cursor=child;const seen=new Set();while(cursor.id!==parent.id){assert.ok(!seen.has(cursor.id));seen.add(cursor.id);cursor=all.find(p=>p.id===cursor.parent_proposal_id);assert.ok(cursor,'repair stays in the reviewed parent lineage');}
 checkReviewedRepair(parent.document,child.document,targets);
 assert.deepEqual(await call('GET',path),formal);assert.deepEqual(await call('GET',`${path}/proposals/${parent.id}`),parent);
 const records=sourceRecords(JSON.parse(await readFile(join(directory,'session-history.json'),'utf8')));
 assert.ok(records.some(r=>r.record?.stId==='R-HSA-5672973'&&/threonine and tyrosine/i.test(JSON.stringify(r.record.summation))),'model actually rereads the conflicting source');
 const calls=turns[0].calls.filter(c=>!c.is_error&&c.name==='visualization');
 for(const id of [parent.id,child.id])assert.ok(calls.some(c=>c.input.action==='read_proposal_preview'&&c.input.proposal_id===id));
 let png;await until(async()=>{png=await call('GET',`${path}/proposals/${child.id}/preview?expected_revision=${revision}`);assert.notEqual(png.render_status,'failed');return png.render_status==='ready';},'reviewed repair PNG');
 assert.equal(png.document_sha256,child.document_sha256);await writeFile(join(directory,'reviewed-repair-candidate.png'),Buffer.from(png.dataURL.split(',')[1],'base64'));
 await page.getByRole('tab',{name:'Live figure',exact:true}).click();await page.getByRole('button',{name:'查看 Agent 提案',exact:true}).click();
 const items=(await call('GET',`${path}/proposal-index?limit=20`)).items,matching=items.filter(p=>p.summary===child.summary&&p.base_revision===revision&&p.parent_proposal_id);
 await dialog.getByRole('button',{name:`${child.summary} · v${revision} · 修订`,exact:true}).nth(matching.findIndex(p=>p.id===child.id)).click();await dialog.getByRole('region',{name:'候选画布'}).locator('.excalidraw').waitFor();
 await page.screenshot({path:join(directory,'reviewed-repair-adoption.png'),animations:'disabled'});await dialog.getByRole('button',{name:'采用提案',exact:true}).click();await dialog.waitFor({state:'hidden'});
 const adopted=await call('GET',path);await persist('final.json',adopted);assert.equal(adopted.metadata.revision,revision+1);assert.deepEqual(adopted.scene.document,child.document);
 const download=page.waitForEvent('download');await page.getByRole('button',{name:'导出 源文件',exact:true}).click();const exported=JSON.parse(await readFile(await(await download).path(),'utf8'));await persist('reviewed-repair-source.json',exported);checkPortableSceneExport(exported,child.document);
 return {scope:'explicit reviewer-directed ERK source-conflict and owned-label crossing repair on a supplied saved figure; preserves unrelated objects and historical plans; no autonomous scientific review claim',parent_proposal_id:parent.id,final_proposal_id:child.id};
}
