import {readPlanPreparedContext,readPlanPreparedPrompt} from './canvas-prepared-context.mjs';
import assert from 'node:assert/strict';
import {readFile} from 'node:fs/promises';

// Explicit prior plan, no scene import or fresh science claim. Actual production GUI/model.
export async function runLivePlanReview({page,call,path,send,persist,turns,until,source}) {
  const focused=process.env.VISUALIZATION_LIVE_PLAN_REVIEW_FOCUS==='1';
  const typed=process.env.VISUALIZATION_LIVE_PLAN_REVIEW_TYPED==='1';
  const batch=process.env.VISUALIZATION_LIVE_PLAN_REVIEW_BATCH==='1';
  const indices=batch?[0,3]:[0];
  const editReadonly=process.env.VISUALIZATION_LIVE_PLAN_REVIEW_EDIT_READONLY==='1';
  const saved=JSON.parse(await readFile(source,'utf8')),document=saved.document??saved;
  assert.match(document.edges[0].label,/Release of Cytochrome c/,'explicit reviewed source plan required');
  // Initial Excalidraw restoration can normalize an otherwise empty scene.
  // Save that user-visible state before defining the read-only baseline.
  await page.getByRole('button',{name:'保存画布',exact:true}).click();
  await page.getByRole('button',{name:'保存画布',exact:true}).waitFor();
  const formal=await call('GET',path),revision=formal.metadata.revision;
  assert.equal(formal.scene.document.elements.length,0);
  const initial=await call('PUT',`${path}/planning`,{expected_revision:revision,expected_plan_version:0,document});
  await persist('review-original-plan.json',initial);
  async function prepare(request,name){
    await page.getByRole('tab',{name:'Live figure',exact:true}).click();
    await page.getByRole('button',{name:'绘图计划',exact:true}).click();
    const dialog=page.getByRole('dialog',{name:'科研绘图计划',exact:true});
    await until(async()=>await dialog.getByRole('textbox',{name:'核心图意',exact:true}).inputValue()===document.message,'saved plan loaded');
    const feedback=dialog.locator('details[aria-label="计划标签审阅反馈"]');await feedback.locator('summary').click();
    const selector=feedback.getByRole('combobox',{name:'计划审阅 Agent',exact:true});
    const targets=await selector.locator('option').evaluateAll(options=>options.map(o=>o.value).filter(Boolean));assert.equal(targets.length,1);await selector.selectOption(targets[0]);
    if(focused)for(const index of indices)await feedback.getByRole('checkbox',{name:`审阅 edges[${index}].label`,exact:true}).check();
    const preparedRequest=editReadonly&&name==='plan-review-readonly-request.json'?`请把已勾选的关系标签「${document.edges[0].label}」改为「释放（由线粒体膜间隙至胞质）」并保存一版计划。保留其他字段，不生成图稿。`:request;
    await feedback.getByRole('textbox',{name:'计划标签审阅要求',exact:true}).fill(preparedRequest);
    await feedback.getByRole('button',{name:'准备计划审阅草稿',exact:true}).click();
    const composer=page.getByPlaceholder('输入消息…');await composer.waitFor();const draft=await readPlanPreparedPrompt(page);
    const context=await readPlanPreparedContext(page);assert.deepEqual(context.selected_plan_fields,focused?indices.map(i=>`edges[${i}].label`):[]);
    if(focused){
      assert.equal(context.long_label_review.labels.length,indices.length);assert.ok(context.long_label_review.unselected_labels>0);
      for(const index of indices){
        const label=context.long_label_review.labels.find(label=>label.plan_field===`edges[${index}].label`);assert.ok(label);
        const edge=document.edges[index],from=document.nodes.find(n=>n.id===edge.from),to=document.nodes.find(n=>n.id===edge.to);
        assert.deepEqual(label.context,{kind:'relation',from_node_id:from.id,to_node_id:to.id,from_panel_id:from.panel??null,to_panel_id:to.panel??null,cross_panel:from.panel&&to.panel?from.panel!==to.panel:null});
      }
    }
    assert.equal(context.planning_version,initial.version);assert.equal(context.expected_revision,revision);assert.equal(context.prepared_request,preparedRequest);assert.ok(context.long_label_review.total>0);assert.ok(context.long_label_review.labels.some(label=>label.plan_field));
    assert.equal(await composer.inputValue(),preparedRequest);if(preparedRequest!==request){await composer.fill(request);assert.deepEqual(await readPlanPreparedContext(page),context);}
    await persist(name,{request,prepared_request:preparedRequest,draft,context});
  }
  await prepare('只读取当前计划，检查长标签诊断并给出简短措辞建议。'+(focused?'先根据所选诊断 context 说明所选关系是否跨面板，不猜测换行原因。':'')+'不要保存计划，不生成草图、不创建候选，也不修改图稿。保留科学内容与来源，本轮只给建议。','plan-review-readonly-request.json');
  await send(turns.length+1);
  assert.ok(turns.at(-1).calls.some(c=>!c.is_error&&c.name==='visualization'&&c.input.action==='read_plan'));
  assert.deepEqual(await call('GET',`${path}/planning`),initial);assert.deepEqual(await call('GET',path),formal);assert.deepEqual(await call('GET',`${path}/proposals`),[]);
  await persist('plan-review-readonly-audit.json',{passed:true,plan_scene_proposals_unchanged:true});
  const replacement='释放（由线粒体膜间隙至胞质）';
  const batchChanges=[{plan_field:'edges[0].label',expected_text:document.edges[0].label,text:replacement},{plan_field:'edges[3].label',expected_text:document.edges[3].label,text:'procaspase-9 二聚化'}];
  if(batch)await prepare(`现在明确授权一次性保存两条已勾选标签修订：第一条「${batchChanges[0].expected_text}」逐字改为「${batchChanges[0].text}」；第二条「${batchChanges[1].expected_text}」逐字改为「${batchChanges[1].text}」。这是本轮明确审阅的措辞，先读计划核对版本，用一次批量标签局部更新保存，只产生一个新计划版本，不要回传完整计划。其他所有字段逐字段保持，包括其他标签、节点、关系、方向、依据、假设、问题ID与答案。不生成草图或候选，不修改正式图。简短报告新版本。`,'plan-review-edit-request.json');
  else await prepare(`现在明确授权保存一条标签修订：只将${focused?'已勾选标签':' edges[0].label'}的「${document.edges[0].label}」逐字改为「${replacement}」。这是本轮已审阅措辞，先读计划核对版本，${typed?'使用单标签局部更新保存一次，不要回传完整计划':'使用 save_plan 保存一次'}。其他所有字段逐字段保持，包括所有节点、其他关系、方向、依据、假设和问题ID/答案。不生成草图、不创建候选，不修改正式图。完成后简短报告新计划版本。`,'plan-review-edit-request.json');
  await send(turns.length+1);
  const updated=await call('GET',`${path}/planning`),expected=structuredClone(document);expected.edges[0].label=replacement;if(batch)expected.edges[3].label=batchChanges[1].text;
  assert.equal(updated.version,initial.version+1);assert.equal(updated.based_on_revision,revision);assert.deepEqual(updated.document,expected);
  const writes=turns.at(-1).calls.filter(c=>!c.is_error&&(c.name==='visualization_update_plan_labels'||c.name==='visualization_update_plan_label'||(c.name==='visualization'&&c.input.action==='save_plan')));assert.equal(writes.length,1);assert.equal(writes[0].input.planning_version,initial.version);assert.equal(writes[0].input.expected_revision,revision);if(batch){assert.equal(writes[0].name,'visualization_update_plan_labels');assert.deepEqual([...writes[0].input.changes].sort((a,b)=>a.plan_field.localeCompare(b.plan_field)),batchChanges);assert.ok(!Object.hasOwn(writes[0].input,'planning')&&!Object.hasOwn(writes[0].input,'document'));await persist('plan-review-batch-write.json',{name:writes[0].name,input:writes[0].input,input_utf8_bytes:Buffer.byteLength(JSON.stringify(writes[0].input))});}else if(typed){assert.equal(writes[0].name,'visualization_update_plan_label');assert.equal(writes[0].input.plan_field,'edges[0].label');assert.equal(writes[0].input.expected_text,document.edges[0].label);assert.equal(writes[0].input.text,replacement);assert.ok(!Object.hasOwn(writes[0].input,'planning')&&!Object.hasOwn(writes[0].input,'document'));await persist('plan-review-typed-write.json',{name:writes[0].name,input:writes[0].input,input_utf8_bytes:Buffer.byteLength(JSON.stringify(writes[0].input))});}
  assert.deepEqual((await call('GET',`${path}/planning/revisions/${initial.version}`)).document,document);
  assert.deepEqual(await call('GET',path),formal);assert.deepEqual(await call('GET',`${path}/proposals`),[]);await persist('review-final-plan.json',updated);
  return {edited_readonly_overrides_prepared_write:editReadonly,scope:`actual GUI/model reviews explicit prior scientific plan without writing, then saves ${batch?'two explicitly authorized labels in one version':'one explicitly authorized label'}; no scene import or new scientific generation`,batch_label_update_verified:batch,typed_label_update_verified:typed,selected_label_scope_verified:focused,readonly_verified:true,one_label_revision_verified:!batch,unrelated_plan_fields_preserved:true,original_plan_history_preserved:true,formal_scene_unchanged:true};
}
