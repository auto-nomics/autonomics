import assert from 'node:assert/strict';
import {readFile} from 'node:fs/promises';
import {readCandidatePreparedContext} from './canvas-prepared-context.mjs';
export async function runLiveBatchSearch({page,call,path,send,persist,turns,source}){
 const input=JSON.parse(await readFile(source,'utf8')),document=input.scene?.document??input.document??input;
 const initial=await call('GET',path);await call('PUT',path,{expected_revision:initial.metadata.revision,title:'Live figure',document});page.once('dialog',d=>d.accept());await page.getByRole('button',{name:'重新加载服务端版本',exact:true}).click();await page.locator('[data-canvas-surface] .excalidraw:visible').waitFor();
 const formal=await call('GET',path),revision=formal.metadata.revision,seed=await call('POST',`${path}/proposals`,{expected_revision:revision,summary:'只读检索多个通用图形概念',operations:[]}),proposals=await call('GET',`${path}/proposals`);
 await page.getByRole('button',{name:'查看 Agent 提案',exact:true}).click();const dialog=page.getByRole('dialog',{name:'Agent 修改预览',exact:true});await dialog.locator(`button[data-proposal-id="${seed.id}"]`).click();await dialog.getByRole('region',{name:'候选画布',exact:true}).locator('.excalidraw').waitFor();
 const feedback=dialog.locator('details[aria-label="候选修订反馈"]');await feedback.locator('summary').click();
 const request='本轮仅做BioIcons目录检索，不修改图稿或计划，不生成候选，不采用或拒绝。请一次检索三个独立英文关键词 receptor、protein、nucleus，分别报告命中总数与返回结果数量，再用一句话说明这些词法结果为什么不能证明具体分子结构适用或目录不存在某组件。不要插入、替换、下载或预览SVG，不需要读取图稿或科学文献；当前候选仅作背景。用简短中文回复，不重复内部ID与工具过程。';
 await feedback.getByRole('textbox',{name:'候选修订要求',exact:true}).fill(request);await feedback.getByRole('button',{name:'准备候选修订草稿',exact:true}).click();await page.getByPlaceholder('输入消息…').waitFor();const context=await readCandidatePreparedContext(page);assert.equal(context.proposal_id,seed.id);await persist('batch-search-request.json',{request,context});await send(1);
 const calls=turns[0].calls,searches=calls.filter(c=>!c.is_error&&c.name==='bioicons'&&c.input.action==='search_batch');assert.equal(searches.length,1);assert.deepEqual([...searches[0].input.queries].sort(),['nucleus','protein','receptor']);assert.deepEqual(calls.filter(c=>c.name.startsWith('visualization')||c.name==='bioart'||(c.name==='bioicons'&&c.input.action!=='search_batch')),[]);assert.deepEqual(calls.filter(c=>c.is_error),[]);
 assert.deepEqual(await call('GET',path),formal);assert.deepEqual(await call('GET',`${path}/proposals`),proposals);
 await persist('batch-search-readonly-audit.json',{passed:true,formal_plan_all_proposals_unchanged:true,actual_batch_calls:1,queries:searches[0].input.queries});
 return {case:'live_gui_bioicons_batch_search',readonly_verified:true,actual_batch_calls:1,tool_calls:calls.length,scope:'three explicit generic catalogue terms, no asset selection, SVG preview or scientific suitability verified; semantic response requires independent review'};
}
