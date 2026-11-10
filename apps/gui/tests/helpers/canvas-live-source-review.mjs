// Actual model repairs two explicitly reviewed caption claims in a saved figure.
// This is local scientific-language feedback, not autonomous source discovery.
import assert from 'node:assert/strict';
import {readFile,writeFile} from 'node:fs/promises';
import {createHash} from 'node:crypto';
import {join} from 'node:path';
import {currentCandidateLeaf} from './canvas-live-scientific.mjs';

export const captionCorrections = [
 ['3) IFN-γ 的 STAT1 同源二聚化在 Reactome 无独立反应记录，二聚化与脱离受体合并记于 R-HSA-873927；', '3) 本图按本次已读 R-HSA-873927 将二聚化与受体释放合并描述；未对整个数据库独立二聚化记录的有无作判断；'],
 ['5) Ser727 二次磷酸化未画入本图（R-HSA-909552），GAF 入核后仍可发生。', '5) Ser727 磷酸化未画入本图；所读 R-HSA-909552 说明其并非 STAT1 同源二聚化与核转位所必需，本图不据此推断发生时序或区室。'],
];
export function checkSourceReviewExport(exported, document, expectedText) {
 assert.equal(exported.elements.length,document.elements.length);assert.deepEqual(exported.files,document.files);
 const fields=['type','x','y','width','height','angle','points','startBinding','endBinding','boundElements','groupIds','frameId','text','originalText','fontFamily','fontSize','customData'];
 for(const element of document.elements){const actual=exported.elements.find(e=>e.id===element.id);assert.ok(actual);for(const field of fields)assert.deepEqual(actual[field]??null,element[field]??null,`portable ${element.id}.${field}`);}
 assert.equal(exported.elements.find(e=>e.id==='note_scope_body').originalText,expectedText);
}
export async function runLiveSourceReview({page,call,path,send,persist,directory,turns,until,source}) {
 const bytes=await readFile(source),input=JSON.parse(bytes),document=input.scene?.document??input.document??input;
 const body=document.elements.find(e=>e.id==='note_scope_body'&&e.type==='text');assert.ok(body,'explicit saved source-review fixture');
 const originalText=body.originalText??body.text;let expected=originalText;for(const [before,after] of captionCorrections){assert.ok(expected.includes(before));expected=expected.replace(before,after);}
 const head=await call('GET',path);await call('PUT',path,{expected_revision:head.metadata.revision,title:'Live figure',document});
 page.once('dialog',d=>d.accept());await page.getByRole('button',{name:'重新加载服务端版本',exact:true}).click();await page.locator('[data-canvas-surface] .excalidraw:visible').waitFor();
 const formal=await call('GET',path),revision=formal.metadata.revision;
 assert.deepEqual(formal.scene.document,document);
 const parent=await call('POST',`${path}/proposals`,{expected_revision:revision,summary:'核对图下范围与来源说明',operations:[]});
 await persist('source-review-seed.json',{source_sha256:createHash('sha256').update(bytes).digest('hex'),formal,parent});
 await page.getByRole('button',{name:'查看 Agent 提案',exact:true}).click();const dialog=page.getByRole('dialog',{name:'Agent 修改预览',exact:true});await dialog.getByRole('button',{name:`${parent.summary} · v${revision}`,exact:true}).click();await dialog.getByRole('region',{name:'候选画布'}).locator('.excalidraw').waitFor();
 const feedback=dialog.locator('details[aria-label="候选修订反馈"]');await feedback.locator('summary').click();
 const request=['我已对照本次读取的 Reactome 来源核对图下范围说明。只修改 note_scope_body 中下面两处文字，逐字替换；其他文字、字号、字体、x/y/宽度、节点、连线、全部引用、文件和历史 planningSource 均保持不变。不要重建草图，不增加分子、箭头或来源。',...captionCorrections.map(([before,after],i)=>`${i+1}. 将「${before}」替换为「${after}」。`),'先读取当前父候选及其 PNG，确认目标文字，再在该候选上用 set_wrapped_text 增量修订。读取最终候选 PNG 检查换行和图文关系后留待我审阅，不采用。'].join('\n');
 await feedback.getByRole('textbox',{name:'候选修订要求',exact:true}).fill(request);await feedback.getByRole('button',{name:'准备候选修订草稿',exact:true}).click();await page.getByPlaceholder('输入消息…').waitFor();const draft=await page.getByPlaceholder('输入消息…').inputValue();assert.ok(draft.includes(parent.id)&&draft.includes(parent.document_sha256));await persist('source-review-feedback.json',{request,draft});
 await send(1);const all=await call('GET',`${path}/proposals`);await persist('source-review-all-candidates.json',all);const child=currentCandidateLeaf(all,revision);assert.notEqual(child.id,parent.id);
 let cursor=child;const seen=new Set();while(cursor.id!==parent.id){assert.ok(!seen.has(cursor.id));seen.add(cursor.id);cursor=all.find(p=>p.id===cursor.parent_proposal_id);assert.ok(cursor,'caption revision stays in the reviewed parent lineage');}
 assert.equal(child.document.elements.length,parent.document.elements.length);assert.deepEqual(child.document.files,parent.document.files);
 for(const element of parent.document.elements){const actual=child.document.elements.find(e=>e.id===element.id);assert.ok(actual);if(element.id!=='note_scope_body')assert.deepEqual(actual,element,`unrelated ${element.id} unchanged`);}
 const edited=child.document.elements.find(e=>e.id==='note_scope_body');assert.equal(edited.originalText,expected);
 for(const field of ['x','y','width','fontFamily','fontSize','lineHeight','strokeColor','angle','customData','containerId'])assert.deepEqual(edited[field],body[field],`caption ${field} unchanged`);
 assert.deepEqual(await call('GET',path),formal);assert.deepEqual(await call('GET',`${path}/proposals/${parent.id}`),parent);
 const calls=turns[0].calls.filter(c=>!c.is_error&&c.name==='visualization');assert.ok(calls.some(c=>c.input.action==='read_proposal'&&c.input.proposal_id===parent.id));assert.ok(calls.some(c=>c.input.action==='read_proposal_preview'&&c.input.proposal_id===parent.id));assert.ok(calls.some(c=>c.input.action==='read_proposal_preview'&&c.input.proposal_id===child.id));
 let png;await until(async()=>{png=await call('GET',`${path}/proposals/${child.id}/preview?expected_revision=${revision}`);assert.notEqual(png.render_status,'failed');return png.render_status==='ready';},'reviewed source-caption PNG');assert.equal(png.document_sha256,child.document_sha256);await writeFile(join(directory,'source-review-candidate.png'),Buffer.from(png.dataURL.split(',')[1],'base64'));
 await page.getByRole('tab',{name:'Live figure',exact:true}).click();await page.getByRole('button',{name:'查看 Agent 提案',exact:true}).click();const items=(await call('GET',`${path}/proposal-index?limit=20`)).items,matching=items.filter(p=>p.summary===child.summary&&p.base_revision===revision&&p.parent_proposal_id);await dialog.getByRole('button',{name:`${child.summary} · v${revision} · 修订`,exact:true}).nth(matching.findIndex(p=>p.id===child.id)).click();await dialog.getByRole('region',{name:'候选画布'}).locator('.excalidraw').waitFor();await page.screenshot({path:join(directory,'source-review-adoption.png'),animations:'disabled'});await dialog.getByRole('button',{name:'采用提案',exact:true}).click();await dialog.waitFor({state:'hidden'});
 const adopted=await call('GET',path);assert.equal(adopted.metadata.revision,revision+1);assert.deepEqual(adopted.scene.document,child.document);
 const download=page.waitForEvent('download');await page.getByRole('button',{name:'导出 源文件',exact:true}).click();const exported=JSON.parse(await readFile(await(await download).path(),'utf8'));checkSourceReviewExport(exported,child.document,expected);
 await persist('source-review-source.json',exported);await persist('final.json',adopted);return{scope:'actual Agent applies two explicit retrieval-bound caption corrections to a supplied saved figure; preserves graph, references and historical plan snapshot; no autonomous scientific correctness claim',parent_proposal_id:parent.id,final_proposal_id:child.id};
}
