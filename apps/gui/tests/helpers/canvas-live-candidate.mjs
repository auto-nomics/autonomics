// Real Gateway candidate feedback; no scripted model responses or preview substitutes.
import assert from 'node:assert/strict';
import { readFile, writeFile } from 'node:fs/promises';
import { join } from 'node:path';
export async function runCandidateRevision({page,call,path,send,persist,directory,turns,until}) {
  const normalize=text=>text.replace(/\s/g,'');
  await page.getByRole('button',{name:'保存画布',exact:true}).click();
  await page.getByRole('button',{name:'保存画布',exact:true}).waitFor();
  const formal=await call('GET',path),elements=formal.scene.document.elements;
  const ligand=elements.find(e=>e.type==='text'&&normalize(e.text)==='配体L');
  const response=elements.find(e=>e.type==='text'&&['响应X','细胞响应X'].includes(normalize(e.text)));
  assert.ok(ligand&&response,'explicit illustrated three-component fixture has both labels');
  const parentResult=await call('POST',`${path}/proposals`,{expected_revision:formal.metadata.revision,summary:'保留父候选改动的累积修订验收',operations:[{op:'set_wrapped_text',id:ligand.id,text:'候选配体 L'}]});
  const parent=await call('GET',`${path}/proposals/${parentResult.id}`);
  await persist('candidate-parent.json',parent);await persist('candidate-formal-before.json',formal);
  assert.equal(normalize(parent.document.elements.find(e=>e.id===ligand.id).text),'候选配体L');
  const dialog=page.getByRole('dialog',{name:'Agent 修改预览',exact:true});
  const show=async candidate=>{
    await page.getByRole('tab',{name:'Live figure',exact:true}).click();
    await page.getByRole('button',{name:'查看 Agent 提案',exact:true}).click();
    await dialog.getByRole('button',{name:`${candidate.summary} · v${candidate.base_revision}${candidate.parent_proposal_id?' · 修订':''}`,exact:true}).click();
    await dialog.getByRole('region',{name:'候选画布'}).locator('.excalidraw').waitFor();
  };
  const leaf=async ancestor=>{
    const pending=(await call('GET',`${path}/proposals`)).filter(p=>p.status==='pending');
    const leaves=pending.filter(p=>!pending.some(child=>child.parent_proposal_id===p.id));assert.equal(leaves.length,1,'one unambiguous candidate lineage');
    const seen=new Set();let current=leaves[0];
    while(current){assert.ok(!seen.has(current.id),'no lineage cycle');seen.add(current.id);if(current.id===parent.id)break;assert.ok(current.parent_proposal_id,'new work must derive from a candidate, not the formal scene');current=pending.find(p=>p.id===current.parent_proposal_id);assert.ok(current,'complete candidate lineage is retained');}
    assert.equal(seen.size,pending.length,'no independent ambiguous proposals');assert.ok(seen.has(ancestor.id)&&leaves[0].id!==ancestor.id,'new leaf retains the requested candidate ancestry');return leaves[0];
  };
  const revise=async (candidate,request,number)=>{
    await show(candidate);
    const feedback=dialog.locator('details[aria-label="候选修订反馈"]');await feedback.locator('summary').click();
    await feedback.getByRole('textbox',{name:'候选修订要求',exact:true}).fill(request);
    const count=turns.length;await feedback.getByRole('button',{name:'准备候选修订草稿',exact:true}).click();
    const composer=page.getByPlaceholder('输入消息…');await composer.waitFor();const draft=await composer.inputValue();
    assert.ok(draft.includes(candidate.id)&&draft.includes(candidate.document_sha256),'actual feedback draft carries exact candidate identity');
    assert.equal(turns.length,count,'preparing feedback never sends it');assert.deepEqual(await call('GET',path),formal);
    await send(number);
    const calls=turns[number-1].calls.filter(c=>c.name==='visualization');
    assert.ok(calls.some(c=>c.input.action==='read_proposal'&&c.input.proposal_id===candidate.id),'model reads the specified parent');
    assert.ok(calls.some(c=>c.input.action==='revise_proposal'&&c.input.proposal_id===candidate.id&&c.input.document_sha256===candidate.document_sha256),'model revises exact stored candidate hash');
    const next=await leaf(candidate);assert.ok(calls.some(c=>c.input.action==='read_proposal_preview'&&c.input.proposal_id===next.id),'model checks final child vision');
    let preview;await until(async()=>{preview=await call('GET',`${path}/proposals/${next.id}/preview?expected_revision=${formal.metadata.revision}`);assert.notEqual(preview.render_status,'failed');return preview.render_status==='ready';},'exact child PNG completion');
    assert.equal(preview.document_sha256,next.document_sha256);assert.equal(preview.scope,'proposal_candidate');
    await writeFile(join(directory,`candidate-child-${number}.png`),Buffer.from(preview.dataURL.split(',')[1],'base64'));
    assert.deepEqual(await call('GET',path),formal);assert.deepEqual(await call('GET',`${path}/proposals/${candidate.id}`),candidate,'parent remains immutable and pending');
    assert.deepEqual(next.document.files,parent.document.files);assert.equal(next.document.elements.length,parent.document.elements.length);
    for(const element of parent.document.elements.filter(e=>e.id!==response.id))assert.deepEqual(next.document.elements.find(e=>e.id===element.id),element,`unrelated ${element.id} is preserved including the first parent edit`);
    const beforeText=candidate.document.elements.find(e=>e.id===response.id),afterText=next.document.elements.find(e=>e.id===response.id);
    for(const key of ['fontFamily','lineHeight','textAlign','verticalAlign','containerId','strokeColor','backgroundColor','opacity','groupIds','frameId','customData','angle','locked'])assert.deepEqual(afterText[key],beforeText[key],`response ${key} remains unchanged`);
    if(number===1)assert.equal(afterText.fontSize,beforeText.fontSize,'text-only edit preserves font size');
    await persist(`candidate-child-${number}.json`,next);return next;
  };
  const first=await revise(parent,'只在这份候选上，将“细胞响应 X”（若原标签是“响应 X”则为同一响应组件）的标签改为“候选响应 Y”。保留父候选已有的“候选配体 L”及其他所有对象、图像、连线、位置和科学含义，不更改正式图稿。生成子候选并读取其独立预览。',1);
  const firstText=first.document.elements.find(e=>e.id===response.id);assert.equal(normalize(firstText.text),'候选响应Y');assert.equal(normalize(firstText.originalText),'候选响应Y');
  const size=firstText.fontSize+2;assert.ok(size>=4&&size<=200);
  const second=await revise(first,`继续在当前子候选上，只把“候选响应 Y”的字号明确改为 ${size}，保持该文字及之前“候选配体 L”的改动，不改变其他对象。保留字体和行高，核对候选预览后交给我审阅，不采用。`,2);
  const secondText=second.document.elements.find(e=>e.id===response.id);assert.equal(normalize(secondText.text),'候选响应Y');assert.equal(normalize(secondText.originalText),'候选响应Y');assert.equal(secondText.fontSize,size);assert.equal(secondText.fontFamily,firstText.fontFamily);assert.equal(secondText.lineHeight,firstText.lineHeight);
  await show(second);await dialog.getByRole('button',{name:'对照上一版候选',exact:true}).click();await page.screenshot({path:join(directory,'candidate-comparison.png'),animations:'disabled'});
  await dialog.getByRole('button',{name:'采用提案',exact:true}).click();await dialog.waitFor({state:'hidden'});
  const adopted=await call('GET',path);assert.equal(adopted.metadata.revision,formal.metadata.revision+1);assert.deepEqual(adopted.scene.document,second.document);assert.deepEqual(await call('GET',`${path}/proposals/${parent.id}`),parent);assert.deepEqual(await call('GET',`${path}/proposals/${first.id}`),first);
  await page.getByRole('button',{name:'重新加载服务端版本',exact:true}).click();const download=page.waitForEvent('download');await page.getByRole('button',{name:'导出 源文件',exact:true}).click();const exported=JSON.parse(await readFile(await(await download).path(),'utf8'));
  assert.equal(normalize(exported.elements.find(e=>e.id===ligand.id).text),'候选配体L');assert.equal(normalize(exported.elements.find(e=>e.id===response.id).text),'候选响应Y');assert.equal(exported.elements.find(e=>e.id===response.id).fontSize,size);assert.equal(exported.elements.length,second.document.elements.length);
  await persist('final.json',adopted);await page.screenshot({path:join(directory,'figure.png'),animations:'disabled'});
  return {parent_id:parent.id,first_child_id:first.id,adopted_child_id:second.id,checks:['actual feedback draft with exact hash','two real Agent candidate revisions','cumulative edits and unrelated objects preserved','independent child PNGs','GUI comparison/adoption/reload','immutable pending ancestors']};
}
