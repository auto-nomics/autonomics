import assert from 'node:assert/strict';
import {readFile,writeFile} from 'node:fs/promises';
import {join} from 'node:path';
import {readCandidatePreparedContext} from './canvas-prepared-context.mjs';
import {currentCandidateLeaf} from './canvas-live-scientific.mjs';
import {checkPortableSceneExport} from './canvas-live-source-review.mjs';
const omit=(value,keys)=>Object.fromEntries(Object.entries(value).filter(([key])=>!keys.includes(key)));
// Explicit reviewer instructions on an actual saved figure, not autonomous science.
export async function runLiveResidueReview({page,call,path,send,persist,directory,turns,until,source}){
 const input=JSON.parse(await readFile(source,'utf8')),document=input.scene?.document??input.document??input;
 const replacements=[['磷酸化 MEK 活化环双丝氨酸（S218/S222、S222/S226）','磷酸化 MEK 活化环'],['磷酸化 ERK 活化环 Thr/Tyr（T202/Y204、T185/Y187）','磷酸化 ERK 活化环 Thr/Tyr']];
 const targets=replacements.map(([before,after])=>{const matches=document.elements.filter(e=>e.type==='text'&&!e.isDeleted&&e.originalText===before);assert.equal(matches.length,1);assert.ok(matches[0].containerId);return {id:matches[0].id,before,after};});
 const head=await call('GET',path);await call('PUT',path,{expected_revision:head.metadata.revision,title:'Live figure',document});
 page.once('dialog',d=>d.accept());await page.getByRole('button',{name:'重新加载服务端版本',exact:true}).click();
 const formal=await call('GET',path),revision=formal.metadata.revision;assert.deepEqual(formal.scene.document,document);
 const parent=await call('POST',`${path}/proposals`,{expected_revision:revision,summary:'审阅关系标签的亚型残基归属',operations:[]});
 await persist('residue-review-seed.json',{formal,parent,targets});
 await page.getByRole('button',{name:'查看 Agent 提案',exact:true}).click();const dialog=page.getByRole('dialog',{name:'Agent 修改预览',exact:true});
 await dialog.getByRole('button',{name:`${parent.summary} · v${revision}`,exact:true}).click();await dialog.getByRole('region',{name:'候选画布'}).locator('.excalidraw').waitFor();
 const request=['这份已保存图稿的两条关系标签并列列出了不同亚型的残基编号，却没有逐一标明归属。我本轮选择保留抽象活化状态，不在图上展示具体位点。仅逐字修改以下两份文字：',...targets.map(t=>`${t.id}：「${t.before}」改为「${t.after}」。`),'沿本待审候选生成子候选，用 set_wrapped_text 传完整 text，保留字号与绑定关系，绑定箭头标签不传 max_width；只允许这两份文字在原绑定箭头中心按原宽度重新换行，禁止移动箭头、节点、素材、其他文字或修改证据/文件/历史计划，不添加亚型映射，不重建图稿。先读取父候选和 PNG，修改后读取新候选及 PNG 核对，留待人工审阅，不采用。简短中文报告。'].join('\n');
 const feedback=dialog.locator('details[aria-label="候选修订反馈"]');await feedback.locator('summary').click();await feedback.getByRole('textbox',{name:'候选修订要求',exact:true}).fill(request);await feedback.getByRole('button',{name:'准备候选修订草稿',exact:true}).click();
 await page.getByPlaceholder('输入消息…').waitFor();const context=await readCandidatePreparedContext(page);assert.equal(context.proposal_id,parent.id);assert.equal(context.document_sha256,parent.document_sha256);await persist('residue-review-request.json',{request,context});await send(1);
 const all=await call('GET',`${path}/proposals`),child=currentCandidateLeaf(all,revision);assert.notEqual(child.id,parent.id);
 let cursor=child;const seen=new Set();while(cursor.id!==parent.id){assert.ok(!seen.has(cursor.id));seen.add(cursor.id);cursor=all.find(p=>p.id===cursor.parent_proposal_id);assert.ok(cursor);}
 assert.deepEqual(omit(child.document,['elements']),omit(document,['elements']));assert.equal(child.document.elements.length,document.elements.length);
 const changedFields=['version','versionNonce','updated','text','originalText','x','y','width','height','autoResize'];
 for(const old of document.elements){const actual=child.document.elements.find(e=>e.id===old.id);assert.ok(actual);const target=targets.find(t=>t.id===old.id);if(!target){assert.deepEqual(actual,old,`unrelated ${old.id}`);continue;}assert.equal(actual.originalText,target.after);assert.deepEqual(omit(actual,changedFields),omit(old,changedFields));assert.ok([actual.x,actual.y,actual.width,actual.height].every(Number.isFinite));assert.equal(actual.width,old.width);}
 assert.deepEqual(await call('GET',path),formal);assert.deepEqual(await call('GET',`${path}/proposals/${parent.id}`),parent);
 const calls=turns[0].calls.filter(c=>!c.is_error&&c.name==='visualization');for(const id of [parent.id,child.id])assert.ok(calls.some(c=>c.input.action==='read_proposal_preview'&&c.input.proposal_id===id));
 let png;await until(async()=>{png=await call('GET',`${path}/proposals/${child.id}/preview?expected_revision=${revision}`);assert.notEqual(png.render_status,'failed');return png.render_status==='ready';},'residue review PNG');assert.equal(png.document_sha256,child.document_sha256);await writeFile(join(directory,'residue-review-candidate.png'),Buffer.from(png.dataURL.split(',')[1],'base64'));await persist('residue-review-child.json',child);
 await page.getByRole('tab',{name:'Live figure',exact:true}).click();await page.getByRole('button',{name:'查看 Agent 提案',exact:true}).click();const items=(await call('GET',`${path}/proposal-index?limit=20`)).items,matching=items.filter(p=>p.summary===child.summary&&p.base_revision===revision&&p.parent_proposal_id);await dialog.getByRole('button',{name:`${child.summary} · v${revision} · 修订`,exact:true}).nth(matching.findIndex(p=>p.id===child.id)).click();await dialog.getByRole('region',{name:'候选画布'}).locator('.excalidraw').waitFor();await dialog.getByRole('button',{name:'采用提案',exact:true}).click();await dialog.waitFor({state:'hidden'});
 const final=await call('GET',path);assert.equal(final.metadata.revision,revision+1);assert.deepEqual(final.scene.document,child.document);const download=page.waitForEvent('download');await page.getByRole('button',{name:'导出 源文件',exact:true}).click();const exported=JSON.parse(await readFile(await(await download).path(),'utf8'));checkPortableSceneExport(exported,child.document);for(const t of targets)assert.equal(exported.elements.find(e=>e.id===t.id).originalText,t.after);await persist('residue-review-source.json',exported);await persist('final.json',final);
 return {scope:'explicit reviewer chooses abstract activation labels on saved EGFR figure; two bound arrow labels only, not autonomous scientific review',target_ids:targets.map(t=>t.id),unrelated_elements_and_files_preserved:true,parent_and_formal_preserved_before_adoption:true,adoption_and_portable_export_verified:true};
}
