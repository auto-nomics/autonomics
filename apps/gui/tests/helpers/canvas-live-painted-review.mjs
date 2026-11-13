import assert from 'node:assert/strict';
import {readFile} from 'node:fs/promises';
import {readCandidatePreparedContext} from './canvas-prepared-context.mjs';
export async function runLivePaintedReview({page,call,path,send,persist,turns,source}){
 const input=JSON.parse(await readFile(source,'utf8')),document=input.scene?.document??input.document??input;
 const initial=await call('GET',path);await call('PUT',path,{expected_revision:initial.metadata.revision,title:'Live figure',document});page.once('dialog',d=>d.accept());await page.getByRole('button',{name:'重新加载服务端版本',exact:true}).click();await page.locator('[data-canvas-surface] .excalidraw:visible').waitFor();
 const formal=await call('GET',path),revision=formal.metadata.revision,planning=await call('GET',`${path}/planning`);
 const seed=await call('POST',`${path}/proposals`,{expected_revision:revision,summary:'只读核对透明留白诊断',operations:[]}),proposals=await call('GET',`${path}/proposals`);
 await page.getByRole('button',{name:'查看 Agent 提案',exact:true}).click();const dialog=page.getByRole('dialog',{name:'Agent 修改预览',exact:true});await dialog.locator(`button[data-proposal-id="${seed.id}"]`).click();await dialog.getByRole('region',{name:'候选画布',exact:true}).locator('.excalidraw').waitFor();
 const feedback=dialog.locator('details[aria-label="候选修订反馈"]');await feedback.locator('summary').click();
 const request='本轮只读审阅，不授权任何写入。请读取当前候选并查看对应PNG，用简短中文说明图片视口与采样图形的标签重叠检查分别提示什么，是否足以证明实际遮挡，是否需要仅为透明留白移动对象；也指出这些检查对科学含义和整体布局质量的局限。不要将零采样提示解释为全部科学内容或排版通过。无需外部检索，不保存计划、不生成候选、不修改、不采用或拒绝。';
 await feedback.getByRole('textbox',{name:'候选修订要求',exact:true}).fill(request);await feedback.getByRole('button',{name:'准备候选修订草稿',exact:true}).click();await page.getByPlaceholder('输入消息…').waitFor();
 const context=await readCandidatePreparedContext(page),check=context.candidate_sampled_illustration_label_check;assert.equal(check.available,true);assert.equal(check.supported_images,8);assert.equal(check.complete,true);assert.equal(check.viewport.count,3);assert.equal(check.sampled.count,0);await persist('painted-review-request.json',{request,context});
 await send(turns.length+1);const calls=turns.at(-1).calls,allowed=new Set(['read','read_plan','read_proposal','read_proposal_preview','read_preview','list_proposals','image_info']);
 assert.ok(calls.some(c=>!c.is_error&&c.name==='visualization'&&c.input.action==='read_proposal'&&c.input.proposal_id===seed.id));assert.ok(calls.some(c=>!c.is_error&&c.name==='visualization'&&c.input.action==='read_proposal_preview'&&c.input.proposal_id===seed.id));
 assert.deepEqual(calls.filter(c=>c.name.startsWith('visualization')&&(c.name!=='visualization'||!allowed.has(c.input.action))),[]);assert.deepEqual(calls.filter(c=>c.is_error),[]);assert.deepEqual(await call('GET',path),formal);assert.deepEqual(await call('GET',`${path}/planning`),planning);assert.deepEqual(await call('GET',`${path}/proposals`),proposals);
 await persist('painted-readonly-audit.json',{passed:true,formal_plan_all_proposals_unchanged:true,viewport_pairs:3,sampled_pairs:0});
 return {case:'live_gui_sampled_paint_readonly_review',readonly_verified:true,scope:'real GUI/model review of saved source and sampled geometry diagnostics; final semantic response requires independent review, no fresh scientific generation'};
}
