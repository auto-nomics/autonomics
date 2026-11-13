import {createHash} from 'node:crypto';
import {sampledIllustrationLabelOverlaps} from '../../src/features/canvas/canvas-illustration-overlaps.ts';
import assert from 'node:assert/strict';
import {readFile,writeFile} from 'node:fs/promises';
import {join} from 'node:path';
import {readCandidatePreparedContext} from './canvas-prepared-context.mjs';
import {canvasMaterializationProgress} from '../../src/features/canvas/canvas-selection-context.ts';
import {currentCandidateLeaf} from './canvas-live-scientific.mjs';
const omit=(v,keys)=>Object.fromEntries(Object.entries(v??{}).filter(([k])=>!keys.includes(k)));
const length=e=>e.points.slice(1).reduce((n,p,i)=>n+Math.abs(p[0]-e.points[i][0])+Math.abs(p[1]-e.points[i][1]),0);
export async function runLiveStyleAuthor({page,call,path,send,persist,directory,turns,source,until}){
 const input=JSON.parse(await readFile(source,'utf8')),document=input.document??input.scene?.document??input;
 const frames=document.elements.filter(e=>e.type==='frame'&&!e.isDeleted).sort((a,b)=>a.x-b.x);assert.equal(frames.length,3);
 const initial=await call('GET',path);await call('PUT',path,{expected_revision:initial.metadata.revision,title:'Live figure',document});page.once('dialog',d=>d.accept());await page.getByRole('button',{name:'重新加载服务端版本',exact:true}).click();await page.locator('[data-canvas-surface] .excalidraw:visible').waitFor();
 const formal=await call('GET',path),revision=formal.metadata.revision,planning=await call('GET',`${path}/planning`),brief=await call('POST',`${path}/proposals`,{expected_revision:revision,summary:'审阅质膜受体的同来源素材搭配',operations:[]}),parent=await call('GET',`${path}/proposals/${brief.id}`);await persist('style-author-parent.json',parent);
 await page.getByRole('button',{name:'查看 Agent 提案',exact:true}).click();const dialog=page.getByRole('dialog',{name:'Agent 修改预览',exact:true});await dialog.locator(`button[data-proposal-id="${parent.id}"]`).click();await dialog.getByRole('region',{name:'候选画布',exact:true}).locator('.excalidraw').waitFor();
 const feedback=dialog.locator('details[aria-label="候选修订反馈"]');await feedback.locator('summary').click();
 const request='请只改善质膜上的两个受体组件（node-1、node-2）的素材画风，使它们与已有Servier蛋白图元更协调。先读取当前候选及PNG；用BioIcons检索时明确限定作者Servier，预览后选择适合作为通用受体示意的素材，再替换这两个组件的主图并按当前实际像素重新贴合。找不到合适素材时如实说明，不扩大到其他作者，不把七跨膜/GPCR、钙感受受体、离子通道或蛋白质构象当作TGFBR结构。图中已有通用素材限定说明保持，科学身份由原文字决定。完整保留所有原文字、字体、字号、科学关系方向、证据、历史计划、三个分面与另外六个组件；只允许目标主图及其透明锚点、紧连的关系线端点作必要几何调整，标签位置保持。两个新素材均保留真实作者、逐图许可和来源。检查最终PNG、文字遮挡和素材归属后生成待审子候选，不采用，不标记意见完成。无需重新检索科学文献或重画流程。用两三句话说明换了什么和仍需我核对的地方。';
 await feedback.getByRole('textbox',{name:'候选修订要求',exact:true}).fill(request);await feedback.getByRole('button',{name:'准备候选修订草稿',exact:true}).click();await page.getByPlaceholder('输入消息…').waitFor();const context=await readCandidatePreparedContext(page);assert.equal(context.proposal_id,parent.id);assert.equal(context.document_sha256,parent.document_sha256);await persist('style-author-request.json',{request,context});
 if(process.env.VISUALIZATION_STYLE_EFFORT==='low'){await page.getByRole('button',{name:'推理力度',exact:true}).click();await page.locator('[data-composer-popover]').getByRole('button').filter({has:page.getByText('低',{exact:true})}).click();const actors=(await call('GET','/api/v1/state')).agents;await until(async()=>{const config=await call('GET',`/api/v1/agents/${encodeURIComponent(actors[0].path)}/model`);return config.reasoning_effort==='low';},'isolated style Agent effort');}
 await send(1);
 const calls=turns[0].calls,successful=calls.filter(c=>!c.is_error),targets=['node-1','node-2'];
 assert.ok(successful.some(c=>c.name==='bioicons'&&['search','search_batch'].includes(c.input.action)&&c.input.author?.trim().toLowerCase()==='servier'),'Agent actually uses exact author filter');
 const all=await call('GET',`${path}/proposals`),leaf=currentCandidateLeaf(all,revision),child=await call('GET',`${path}/proposals/${leaf.id}`);assert.notEqual(child.id,parent.id);await persist('style-author-child.json',child);await persist('candidate-chain.json',all);
 assert.equal(child.document.elements.length,document.elements.length);
 const targetImages=new Set(targets.map(id=>document.elements.find(e=>e.id===id).customData.visualAssetElementId));
 const connected=new Set(document.elements.filter(e=>e.type==='arrow'&&[e.startBinding?.elementId,e.endBinding?.elementId].some(id=>targets.includes(id))).map(e=>e.id));
 for(const old of document.elements){const next=child.document.elements.find(e=>e.id===old.id);assert.ok(next);
 if(targetImages.has(old.id)){assert.notEqual(next.fileId,old.fileId);assert.equal(next.customData.asset.author,'Servier');assert.ok(next.customData.asset.license&&next.customData.asset.source_url);assert.match(next.customData.asset.name,/receptor/i);assert.doesNotMatch(next.customData.asset.name,/(?:[0-9]+helix|gpcr|calcium|channel)/i);assert.equal(next.customData.semanticAnchorId,old.customData.semanticAnchorId);const file=child.document.files[next.fileId];assert.equal(createHash('sha256').update(Buffer.from(file.dataURL.split(',')[1],'base64')).digest('hex'),next.customData.sha256);}
 else if(targets.includes(old.id)){assert.deepEqual(omit(next.customData,['visualAnchorFit','visualAssetSlot']),omit(old.customData,['visualAnchorFit','visualAssetSlot']));assert.equal(next.frameId,old.frameId);}
 else if(connected.has(old.id)){assert.deepEqual(next.customData,old.customData);for(const k of ['startArrowhead','endArrowhead','strokeStyle','strokeColor','strokeWidth'])assert.deepEqual(next[k],old[k]);for(const k of ['startBinding','endBinding'])assert.equal(next[k]?.elementId,old[k]?.elementId);}
 else assert.deepEqual(next,old,'unrelated object unchanged '+old.id);
 }
 for(const [id,file] of Object.entries(document.files))assert.deepEqual(child.document.files[id],file);
 const progress=canvasMaterializationProgress(child.document.elements);assert.equal(progress.counts.snapshot_current,8);assert.equal(progress.label_ownership_issues.total,0);assert.equal(progress.unassigned_images.total,0);await persist('progress.json',progress);
 const receipt=await call('GET',`${path}/proposals/${child.id}/image-bounds?expected_revision=${revision}&document_sha256=${child.document_sha256}`),paint=await sampledIllustrationLabelOverlaps(child.document,receipt.infos);assert.equal(paint.all_image_text.complete,true);assert.equal(paint.all_image_text.sampled.count,0);await persist('paint-review.json',{receipt,paint});
 const review=await call('GET',`${path}/proposals/${child.id}/geometry-review`);assert.equal(review.document_sha256,child.document_sha256);assert.equal(review.scene_geometry_review.text_rectangle_overlap.overlap_count,0);assert.equal(review.scene_geometry_review.text_rectangle_overlap.status,'complete');await persist('geometry-review.json',review);
 assert.deepEqual(await call('GET',path),formal);assert.deepEqual(await call('GET',`${path}/planning`),planning);assert.deepEqual(await call('GET',`${path}/proposals/${parent.id}`),parent);
 const parentPreviewRead=successful.some(c=>c.name==='visualization'&&c.input.action==='read_proposal_preview'&&c.input.proposal_id===parent.id);assert.ok(successful.some(c=>c.name==='visualization'&&c.input.action==='read_proposal_preview'&&c.input.proposal_id===child.id));const preview=await call('GET',`${path}/proposals/${child.id}/preview?expected_revision=${revision}`);assert.equal(preview.render_status,'ready');assert.equal(preview.document_sha256,child.document_sha256);await writeFile(join(directory,'style-author-candidate.png'),Buffer.from(preview.dataURL.split(',')[1],'base64'));
 return {case:'live_style_author',scope:'real Agent explicit two-component author-filtered replacement on an existing full eight-component scene; inspect visual result separately; no adoption or biological structure validation',tool_calls:calls.length,tool_errors:calls.filter(c=>c.is_error).length,parent_preview_read:parentPreviewRead};
}
