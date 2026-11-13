// Actual model starts from a saved figure, without a seeded candidate.
import assert from 'node:assert/strict';
import {readFile,writeFile} from 'node:fs/promises';
import {join} from 'node:path';
import {createHash} from 'node:crypto';
import {currentCandidateLeaf} from './canvas-live-scientific.mjs';
import {retrievalScopeReview,checkSourceReviewExport} from './canvas-live-source-review.mjs';

export async function runLiveFormalSourceReview({page,call,path,handoff,send,persist,directory,turns,until,source}) {
 const bytes=await readFile(source),input=JSON.parse(bytes),document=input.scene?.document??input.document??input;
 const {elementId,corrections}=retrievalScopeReview,body=document.elements.find(e=>e.id===elementId&&e.type==='text');assert.ok(body);
 let expected=body.originalText??body.text;for(const [before,after] of corrections){assert.ok(expected.includes(before));expected=expected.replace(before,after);}
 const head=await call('GET',path);await call('PUT',path,{expected_revision:head.metadata.revision,title:'Live figure',document});
 page.once('dialog',d=>d.accept());await page.getByRole('button',{name:'重新加载服务端版本',exact:true}).click();await page.locator('[data-canvas-surface] .excalidraw:visible').waitFor();
 const formal=await call('GET',path),revision=formal.metadata.revision;assert.deepEqual(formal.scene.document,document);
 assert.deepEqual(await call('GET',`${path}/proposals`),[],'no seeded proposal');
 await persist('formal-review-seed.json',{source_sha256:createHash('sha256').update(bytes).digest('hex'),formal});
 const request=[`我已对照本次读取的来源核对图下说明。只修改正式画布对象 ${elementId} 的一处文字，其他文字、字体字号、x/y/宽度、图形、关系、来源和历史规划都保留。`,...corrections.map(([before,after])=>`将「${before}」逐字替换为「${after}」。`),`这是已保存的正式画布，不是父候选。先用 read 的 element_ids:["${elementId}"] 和当前 expected_revision 核对原始对象，再 read_preview 查看正式画布。用 propose 的 set_wrapped_text 生成待审候选，不直接修改正式图稿，不重建草图。最后查看候选 PNG 并留待审阅，不采用。`].join('\n');
 await handoff(request);const draft=await page.getByPlaceholder('输入消息…').inputValue();await persist('formal-review-feedback.json',{request,draft});
 await send(1);const all=await call('GET',`${path}/proposals`),child=currentCandidateLeaf(all,revision);await persist('formal-review-candidates.json',all);
 assert.match(child.document_sha256,/^[0-9a-f]{64}$/);assert.equal(child.document.elements.length,document.elements.length);
 assert.deepEqual(Object.fromEntries(Object.entries(child.document).filter(([k])=>k!=='elements')),Object.fromEntries(Object.entries(document).filter(([k])=>k!=='elements')));
 for(const prior of document.elements){const actual=child.document.elements.find(e=>e.id===prior.id);assert.ok(actual);if(prior.id!==elementId)assert.deepEqual(actual,prior);}
 const edited=child.document.elements.find(e=>e.id===elementId);assert.equal(edited.originalText,expected);
 for(const key of ['x','y','width','fontFamily','fontSize','lineHeight','strokeColor','angle','customData','containerId'])assert.deepEqual(edited[key],body[key],key);
 assert.deepEqual(await call('GET',path),formal,'formal figure stays unchanged pending human adoption');
 const calls=turns[0].calls.filter(c=>!c.is_error&&c.name==='visualization'),reads=calls.filter(c=>c.input.action==='read');assert.ok(reads.length>0);
 for(const read of reads){assert.deepEqual(read.input.element_ids,[elementId]);assert.equal(read.input.expected_revision,revision);}
 assert.ok(calls.some(c=>c.input.action==='read_preview'));assert.ok(calls.some(c=>c.input.action==='propose'));
 assert.ok(calls.some(c=>c.input.action==='read_proposal_preview'&&c.input.proposal_id===child.id));
 const history=JSON.parse(await readFile(join(directory,'session-history.json'),'utf8')),uses=new Map(history.flatMap(m=>m.content).filter(b=>b.type==='tool_use').map(b=>[b.id,b]));
 const delivered=[];
 for(const block of history.flatMap(m=>m.content)){
  if(block.type!=='tool_result'||block.is_error)continue;const use=uses.get(block.tool_use_id);if(use?.name!=='visualization'||use.input.action!=='read')continue;
  assert.equal(typeof block.content,'string');const result=JSON.parse(block.content);
  assert.deepEqual(result.selected_elements,[body]);assert.equal(result.metadata.revision,revision);assert.equal(result.scope.revision_scope,'complete_immutable_saved_revision');assert.ok(!('document' in result.scene));
  delivered.push({bytes:Buffer.byteLength(block.content),selected_count:result.selected_elements.length,geometry_context_count:result.geometry_context.elements.length,omitted_count:result.geometry_context.omitted_count});
 }
 assert.ok(delivered.length>0);await persist('formal-scoped-read-evidence.json',{passed:true,delivered,scope:'actual tool text responses; no token or latency inference'});
 let png;await until(async()=>{png=await call('GET',`${path}/proposals/${child.id}/preview?expected_revision=${revision}`);assert.notEqual(png.render_status,'failed');return png.render_status==='ready';},'formal local edit candidate PNG');assert.equal(png.document_sha256,child.document_sha256);
 await writeFile(join(directory,'formal-review-candidate.png'),Buffer.from(png.dataURL.split(',')[1],'base64'));
 await page.getByRole('tab',{name:'Live figure',exact:true}).click();await page.getByRole('button',{name:'查看 Agent 提案',exact:true}).click();const dialog=page.getByRole('dialog',{name:'Agent 修改预览',exact:true});
 await dialog.getByRole('button',{name:`${child.summary} · v${revision}${child.parent_proposal_id?' · 修订':''}`,exact:true}).click();await dialog.getByRole('region',{name:'候选画布'}).locator('.excalidraw').waitFor();
 await page.screenshot({path:join(directory,'formal-review-adoption.png'),animations:'disabled'});await dialog.getByRole('button',{name:'采用提案',exact:true}).click();await dialog.waitFor({state:'hidden'});
 const adopted=await call('GET',path);assert.equal(adopted.metadata.revision,revision+1);assert.deepEqual(adopted.scene.document,child.document);
 const download=page.waitForEvent('download');await page.getByRole('button',{name:'导出 源文件',exact:true}).click();const exported=JSON.parse(await readFile(await(await download).path(),'utf8'));checkSourceReviewExport(exported,child.document,expected,elementId);
 await persist('formal-review-source.json',exported);await persist('final.json',adopted);
 return {scope:'actual Agent edits an explicitly reviewed caption in a supplied saved figure using scoped formal read and propose; no seeded candidate or autonomous scientific review',corrected_element_id:elementId,scoped_formal_read_verified:true,final_proposal_id:child.id};
}
