import {sampledIllustrationLabelOverlaps} from '../../src/features/canvas/canvas-illustration-overlaps.ts';
import assert from 'node:assert/strict';
import {readFile,writeFile} from 'node:fs/promises';
import {join} from 'node:path';
import {readCandidatePreparedContext} from './canvas-prepared-context.mjs';
import {canvasMaterializationProgress} from '../../src/features/canvas/canvas-selection-context.ts';
import {currentCandidateLeaf} from './canvas-live-scientific.mjs';
const omit=(v,keys)=>Object.fromEntries(Object.entries(v??{}).filter(([k])=>!keys.includes(k)));
export async function runLiveCropRepair({page,call,path,send,persist,directory,turns,source,until}){
 const input=JSON.parse(await readFile(source,'utf8')),document=input.document??input.scene?.document??input;
 const frames=document.elements.filter(e=>e.type==='frame'&&!e.isDeleted).sort((a,b)=>a.x-b.x);assert.equal(frames.length,3);
 const initial=await call('GET',path);await call('PUT',path,{expected_revision:initial.metadata.revision,title:'Live figure',document});page.once('dialog',d=>d.accept());await page.getByRole('button',{name:'重新加载服务端版本',exact:true}).click();await page.locator('[data-canvas-surface] .excalidraw:visible').waitFor();
 const formal=await call('GET',path),revision=formal.metadata.revision,brief=await call('POST',`${path}/proposals`,{expected_revision:revision,summary:'审阅质膜受体的同来源素材搭配',operations:[]}),parent=await call('GET',`${path}/proposals/${brief.id}`);await persist('crop-repair-parent.json',parent);
 await page.getByRole('button',{name:'查看 Agent 提案',exact:true}).click();const dialog=page.getByRole('dialog',{name:'Agent 修改预览',exact:true});await dialog.locator(`button[data-proposal-id="${parent.id}"]`).click();await dialog.getByRole('region',{name:'候选画布',exact:true}).locator('.excalidraw').waitFor();
 const feedback=dialog.locator('details[aria-label="候选修订反馈"]');await feedback.locator('summary').click();
 const request='本轮只修复node-2现有的Servier受体主图：它目前显示多受体膜带，请先查看当前候选PNG，再预览实际原SVG，选择其中一个完整的通用受体及少量膜区，使用原生裁剪保留这一部分，按裁剪矩形重新贴合透明锚点，并等比例放大到约70–100个画布单位高，在现有槽位内避免遮挡标签。不下载外部裁剪图，不重新选素材，不手写不完整crop，不新增实验图片/分面或基于正式稿另开分支。保留素材文件和真实来源、全部原文字/字体/字号/标签位置、三分面、另外七个组件、科学关系方向/证据和历史计划；只允许目标主图/锚点及其紧连关系线端点变化。图形仅为通用示意，不推断TGFBR特异结构，原有限定说明保持。检查最终PNG、裁剪范围和遮挡后留下一份沿本候选链的待审子候选，不采用。用两三句话说明实际改动和仍需核对的地方。';
 await feedback.getByRole('textbox',{name:'候选修订要求',exact:true}).fill(request);await feedback.getByRole('button',{name:'准备候选修订草稿',exact:true}).click();await page.getByPlaceholder('输入消息…').waitFor();const context=await readCandidatePreparedContext(page);assert.equal(context.proposal_id,parent.id);assert.equal(context.document_sha256,parent.document_sha256);await persist('crop-repair-request.json',{request,context});
 if(process.env.VISUALIZATION_STYLE_EFFORT==='low'){await page.getByRole('button',{name:'推理力度',exact:true}).click();await page.locator('[data-composer-popover]').getByRole('button').filter({has:page.getByText('低',{exact:true})}).click();const actors=(await call('GET','/api/v1/state')).agents;await until(async()=>{const config=await call('GET',`/api/v1/agents/${encodeURIComponent(actors[0].path)}/model`);return config.reasoning_effort==='low';},'isolated style Agent effort');}
 await send(1);
 const calls=turns[0].calls,successful=calls.filter(c=>!c.is_error),target='node-2';
 assert.ok(successful.some(c=>c.name==='visualization_crop_image'),'Agent uses typed crop tool');
 const all=await call('GET',`${path}/proposals`),leaf=currentCandidateLeaf(all,revision),child=await call('GET',`${path}/proposals/${leaf.id}`);assert.notEqual(child.id,parent.id);await persist('crop-repair-child.json',child);await persist('candidate-chain.json',all);
 const anchor=document.elements.find(e=>e.id===target),image=document.elements.find(e=>e.id===anchor.customData.visualAssetElementId),nextImage=child.document.elements.find(e=>e.id===image.id),connected=new Set(document.elements.filter(e=>e.type==='arrow'&&[e.startBinding?.elementId,e.endBinding?.elementId].includes(target)).map(e=>e.id));
 assert.equal(nextImage.fileId,image.fileId);assert.deepEqual(nextImage.customData,image.customData);assert.ok(nextImage.crop&&nextImage.crop.width/nextImage.crop.naturalWidth<0.5);assert.ok(nextImage.height>=60&&nextImage.height<=120,'cropped artwork is enlarged to readable height');
 assert.deepEqual(child.document.files,document.files);assert.equal(child.document.elements.length,document.elements.length);
 for(const old of document.elements){const next=child.document.elements.find(e=>e.id===old.id);assert.ok(next);
 if(old.id===target){assert.deepEqual(omit(next.customData,['visualAnchorFit','visualAssetSlot']),omit(old.customData,['visualAnchorFit','visualAssetSlot']));assert.equal(next.frameId,old.frameId);}
 else if(old.id===image.id)continue;
 else if(connected.has(old.id)){assert.deepEqual(next.customData,old.customData);for(const k of ['startArrowhead','endArrowhead','strokeStyle','strokeColor','strokeWidth'])assert.deepEqual(next[k],old[k]);for(const k of ['startBinding','endBinding'])assert.equal(next[k]?.elementId,old[k]?.elementId);}
 else assert.deepEqual(next,old,'unrelated object unchanged '+old.id);
 }
 const progress=canvasMaterializationProgress(child.document.elements);assert.equal(progress.counts.snapshot_current,8);assert.equal(progress.counts.needs_fit,0);assert.equal(progress.label_ownership_issues.total,0);assert.equal(progress.unassigned_images.total,0);await persist('progress.json',progress);
 const receipt=await call('GET',`${path}/proposals/${child.id}/image-bounds?expected_revision=${revision}&document_sha256=${child.document_sha256}`),paint=await sampledIllustrationLabelOverlaps(child.document,receipt.infos);assert.equal(paint.all_image_text.complete,true);assert.equal(paint.all_image_text.sampled.count,0);await persist('paint-review.json',{receipt,paint});
 const geometry=await call('GET',`${path}/proposals/${child.id}/geometry-review`);assert.equal(geometry.document_sha256,child.document_sha256);assert.equal(geometry.scene_geometry_review.text_rectangle_overlap.status,'complete');assert.equal(geometry.scene_geometry_review.text_rectangle_overlap.overlap_count,0);await persist('geometry-review.json',geometry);
 assert.deepEqual(await call('GET',path),formal);assert.deepEqual(await call('GET',`${path}/proposals/${parent.id}`),parent);
 assert.ok(successful.some(c=>c.name==='visualization'&&c.input.action==='read_proposal_preview'&&c.input.proposal_id===child.id));const preview=await call('GET',`${path}/proposals/${child.id}/preview?expected_revision=${revision}`);assert.equal(preview.render_status,'ready');assert.equal(preview.document_sha256,child.document_sha256);await writeFile(join(directory,'crop-repair-candidate.png'),Buffer.from(preview.dataURL.split(',')[1],'base64'));
 return {case:'live_crop_repair',scope:'real Agent fixes one saved multi-receptor strip using native source crop, crop-rectangle fit and uniform resize; seven other components and formal remain unchanged; visual and scientific review still required',tool_calls:calls.length,tool_errors:calls.filter(c=>c.is_error).length,height:nextImage.height,source_crop:nextImage.crop};
}
