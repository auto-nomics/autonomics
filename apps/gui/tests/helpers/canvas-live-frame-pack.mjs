import assert from 'node:assert/strict';
import {readFile,writeFile} from 'node:fs/promises';
import {join} from 'node:path';
import {readCandidatePreparedContext} from './canvas-prepared-context.mjs';
import {canvasMaterializationProgress} from '../../src/features/canvas/canvas-selection-context.ts';
import {currentCandidateLeaf} from './canvas-live-scientific.mjs';
const omit=(v,keys)=>Object.fromEntries(Object.entries(v??{}).filter(([k])=>!keys.includes(k)));
const length=e=>e.points.slice(1).reduce((n,p,i)=>n+Math.abs(p[0]-e.points[i][0])+Math.abs(p[1]-e.points[i][1]),0);
export async function runLiveFramePack({page,call,path,send,persist,directory,turns,source}){
 const input=JSON.parse(await readFile(source,'utf8')),document=input.document??input.scene?.document??input;
 const frames=document.elements.filter(e=>e.type==='frame'&&!e.isDeleted).sort((a,b)=>a.x-b.x);assert.equal(frames.length,3);
 const initial=await call('GET',path);await call('PUT',path,{expected_revision:initial.metadata.revision,title:'Live figure',document});page.once('dialog',d=>d.accept());await page.getByRole('button',{name:'重新加载服务端版本',exact:true}).click();await page.locator('[data-canvas-surface] .excalidraw:visible').waitFor();
 const formal=await call('GET',path),revision=formal.metadata.revision,planning=await call('GET',`${path}/planning`),brief=await call('POST',`${path}/proposals`,{expected_revision:revision,summary:'审阅完整素材图的分面间距',operations:[]}),parent=await call('GET',`${path}/proposals/${brief.id}`);await persist('frame-pack-parent.json',parent);
 await page.getByRole('button',{name:'查看 Agent 提案',exact:true}).click();const dialog=page.getByRole('dialog',{name:'Agent 修改预览',exact:true});await dialog.locator(`button[data-proposal-id="${parent.id}"]`).click();await dialog.getByRole('region',{name:'候选画布',exact:true}).locator('.excalidraw').waitFor();
 const feedback=dialog.locator('details[aria-label="候选修订反馈"]');await feedback.locator('summary').click();
 const request='请把当前三个分面按现有左右顺序横向紧凑排列，间距设为144个画布单位。最左分面固定，各分面纵坐标、大小保持，完整素材组件与标签随分面整体平移。允许将这三个分面之间两条关系图注按120单位宽重新自动换行，但必须保留完整originalText原文、字体和字号。其他文字只随所属分面平移，不改文案或尺寸；素材文件、来源、科学关系方向和历史计划保持。读取当前候选与PNG，生成子候选并核对最终PNG及图注遮挡，留待审阅，不采用、不标记意见完成。不要重新规划、重画或检索资料；简短中文报告。';
 await feedback.getByRole('textbox',{name:'候选修订要求',exact:true}).fill(request);await feedback.getByRole('button',{name:'准备候选修订草稿',exact:true}).click();await page.getByPlaceholder('输入消息…').waitFor();const context=await readCandidatePreparedContext(page);assert.equal(context.proposal_id,parent.id);assert.equal(context.document_sha256,parent.document_sha256);await persist('frame-pack-request.json',{request,context});await send(1);
 const calls=turns[0].calls,successful=calls.filter(c=>!c.is_error);assert.ok(successful.some(c=>c.name==='visualization'&&c.input.action==='revise_proposal'&&c.input.operations?.some(o=>o.op==='pack_frames')));
 const all=await call('GET',`${path}/proposals`),leaf=currentCandidateLeaf(all,revision),child=await call('GET',`${path}/proposals/${leaf.id}`);assert.notEqual(child.id,parent.id);assert.equal(child.status,'pending');await persist('frame-pack-child.json',child);
 assert.deepEqual(child.document.files,document.files);assert.equal(child.document.elements.length,document.elements.length);
 const offsets=new Map(frames.map(f=>[f.id,child.document.elements.find(e=>e.id===f.id).x-f.x]));assert.equal(offsets.get(frames[0].id),0);
 for(let i=1;i<frames.length;i++){const a=child.document.elements.find(e=>e.id===frames[i-1].id),b=child.document.elements.find(e=>e.id===frames[i].id);assert.ok(Math.abs(b.x-a.x-a.width-144)<1e-6);}
 const metrics=[];for(const old of document.elements){const next=child.document.elements.find(e=>e.id===old.id);assert.ok(next);const dx=offsets.get(old.id)??offsets.get(old.frameId);if(dx!==undefined){assert.ok(Math.abs(next.x-old.x-dx)<1e-6);assert.equal(next.y,old.y);assert.equal(next.width,old.width);assert.equal(next.height,old.height);}
 assert.deepEqual(omit(next.customData,['visualAnchorFit','visualAssetSlot']),omit(old.customData,['visualAnchorFit','visualAssetSlot']));
 assert.deepEqual(next.groupIds,old.groupIds);
 if(dx===undefined&&old.type!=='arrow'&&!(old.type==='text'&&document.elements.some(e=>e.id===old.containerId&&e.customData?.crossPanel))){assert.deepEqual(next,old);}
 for(const field of ['originalText','fontFamily','fontSize','fileId','frameId'])assert.deepEqual(next[field],old[field]);
 if(old.type==='arrow'){for(const key of ['startBinding','endBinding'])assert.equal(next[key]?.elementId,old[key]?.elementId);assert.deepEqual(next.customData,old.customData);if(old.customData?.crossPanel){metrics.push({id:old.id,before:length(old),after:length(next)});assert.ok(length(next)<length(old));}}
 }
 const progress=canvasMaterializationProgress(child.document.elements);assert.equal(progress.counts.snapshot_current,8);assert.equal(progress.label_ownership_issues.total,0);await persist('progress.json',progress);
 const read=await call('GET',`${path}/proposals/${child.id}/geometry-review`),geometry=read.scene_geometry_review;assert.equal(read.document_sha256,child.document_sha256);assert.equal(geometry.text_rectangle_overlap.status,'complete');assert.equal(geometry.text_rectangle_overlap.overlap_count,0);assert.equal(geometry.caption_path_overlap.warnings.length,0);assert.equal(geometry.caption_path_overlap.skipped_count,0);await persist('geometry-review.json',geometry);
 assert.deepEqual(await call('GET',path),formal);assert.deepEqual(await call('GET',`${path}/planning`),planning);assert.deepEqual(await call('GET',`${path}/proposals/${parent.id}`),parent);
 assert.ok(successful.some(c=>c.name==='visualization'&&c.input.action==='read_proposal_preview'&&c.input.proposal_id===child.id));const preview=await call('GET',`${path}/proposals/${child.id}/preview?expected_revision=${revision}`);assert.equal(preview.render_status,'ready');assert.equal(preview.document_sha256,child.document_sha256);await writeFile(join(directory,'frame-pack-candidate.png'),Buffer.from(preview.dataURL.split(',')[1],'base64'));
 return {case:'live_frame_pack',scope:'real Agent GUI candidate request; explicit three-panel compact layout and authorized caption rewrapping on saved full eight-component scene; no adoption or new scientific claim',tool_calls:calls.length,tool_errors:calls.filter(c=>c.is_error).length,metrics};
}
