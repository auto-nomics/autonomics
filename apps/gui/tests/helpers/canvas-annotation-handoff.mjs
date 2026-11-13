import {canvasMaterializationProgress} from '../../src/features/canvas/canvas-selection-context.ts';
import {readCanvasPreparedContext} from './canvas-prepared-context.mjs';
import assert from 'node:assert/strict';
import {join} from 'node:path';
export async function checkAnnotationHandoff({page,call,project,persist,directory}) {
 const title='Annotation handoff fixture';
 const created=await call('POST','/test/visualization',{action:'sketch',project_id:project,title,document:{message:'GUI annotation handoff fixture',audience:'Testers',layout:'horizontal',nodes:[{id:'a',label:'A'},{id:'b',label:'B'}],edges:[{from:'a',to:'b',label:'Fixture relation'}],assumptions:['No biological claim.']}});
 const path=`/api/v1/visualization/projects/${project}/figures/${created.content.id}`;
 await page.getByRole('button',{name:'刷新画布列表',exact:true}).click();await page.getByRole('button',{name:title,exact:true}).click();
 const surface=page.locator('[data-canvas-surface]:visible .excalidraw');await surface.waitFor();
 const head=await call('GET',path),proposals=await call('GET',`${path}/proposals`);
 await surface.focus();await page.keyboard.press('ControlOrMeta+a');await page.getByRole('button',{name:'批注',exact:true}).click();
 const panel=page.getByRole('dialog',{name:'对象批注',exact:true}),annotations=panel.getByRole('region',{name:'对象批注',exact:true});await panel.waitFor();
 const note='Move the selected fixture objects slightly to the right';await annotations.getByRole('textbox',{name:'批注意见',exact:true}).fill(note);
 const savePrepare=annotations.getByRole('button',{name:'批注并准备 Agent 修改',exact:true});
 let failPreview=true;const pattern='**/revisions/*/preview';
 const handler=async route=>failPreview&&route.request().method()==='PUT'?route.fulfill({status:503,json:{error:'fixture preview failure'}}):route.fallback();await page.route(pattern,handler);
 await savePrepare.click();await annotations.getByRole('alert').filter({hasText:'批注已保存'}).waitFor();
 const saved=await call('GET',`${path}/annotations`);assert.equal(saved.length,1);assert.equal(saved[0].note,note);assert.ok(saved[0].element_ids.length>0);assert.equal(await annotations.getByRole('textbox',{name:'批注意见',exact:true}).inputValue(),'');
 failPreview=false;await page.unroute(pattern,handler);
 const article=annotations.locator('article').filter({hasText:note});await article.getByRole('button',{name:'用此批注准备修改',exact:true}).click();
 const composer=page.locator('textarea[placeholder="输入消息…"]:visible');await composer.waitFor();
 const prompt=await composer.inputValue(),context=await readCanvasPreparedContext(page);
 assert.deepEqual(context.materialization_checkpoint,canvasMaterializationProgress(head.scene.document.elements));assert.equal(context.prepared_request,note);assert.equal(context.focused_annotation_id,saved[0].id);assert.deepEqual(context.annotations,[saved[0]]);assert.deepEqual(new Set(context.selected_objects.map(o=>o.id)),new Set(saved[0].element_ids));
 await page.getByRole('tab',{name:title,exact:true}).click();await page.getByRole('button',{name:'批注',exact:true}).click();
 const secondNote='Adjust the same fixture objects with this separate instruction';await annotations.getByRole('textbox',{name:'批注意见',exact:true}).fill(secondNote);await savePrepare.click();await composer.waitFor();
 const secondPrompt=await composer.inputValue(),secondContext=await readCanvasPreparedContext(page);
 assert.ok(secondPrompt.startsWith(prompt+'\n\n'),'existing Agent draft preserved');assert.equal(secondContext.prepared_request,secondNote);assert.equal(secondContext.annotations.length,1);assert.notEqual(secondContext.focused_annotation_id,saved[0].id);assert.equal((await call('GET',`${path}/annotations`)).length,2);
 await page.getByRole('tab',{name:title,exact:true}).click();await page.getByRole('button',{name:'批注',exact:true}).click();await article.scrollIntoViewIfNeeded();
 const bounds=await panel.boundingBox();for(const button of await article.getByRole('button').all()){const box=await button.boundingBox();assert.ok(box.x>=bounds.x&&box.x+box.width<=bounds.x+bounds.width,'annotation actions fit the floating popover');}
 await page.screenshot({path:join(directory,'annotation-handoff-ready.png'),animations:'disabled'});
 await call('POST',`${path}/annotations/${saved[0].id}/resolve`,{});await article.getByRole('button',{name:'用此批注准备修改',exact:true}).click();await panel.getByRole('alert').filter({hasText:'批注已变化或已处理'}).waitFor();
 await page.screenshot({path:join(directory,'annotation-handoff-stale.png'),animations:'disabled'});
 const final=await call('GET',path);assert.deepEqual(final.scene,head.scene);assert.deepEqual(final.metadata,head.metadata);assert.deepEqual(await call('GET',`${path}/proposals`),proposals);
 await persist('annotation-handoff-review.json',{passed:true,native_selection_floating_entry:true,annotation_actions_fit_popover:true,saved_annotation_retained_on_preview_failure:true,retry_without_duplicate:true,other_open_annotations_excluded:true,existing_agent_draft_preserved:true,exact_annotation_and_objects_in_agent_draft:true,resolved_annotation_rejected:true,formal_and_candidates_unchanged:true});await persist('annotation-handoff-context.json',context);
}
