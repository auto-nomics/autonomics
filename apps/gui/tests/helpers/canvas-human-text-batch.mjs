import assert from 'node:assert/strict';
import {readFile} from 'node:fs/promises';
import {join} from 'node:path';
export async function checkHumanBatchReflow({page,call,project,uiNote,shape,edit,dialog,reselect,persist,directory}) {
 const find=(doc,id)=>doc.elements.find(e=>e.id===id);
 const note=(id,x,originalText)=>({...uiNote,id,x,y:0,width:160,height:75,fontSize:20,lineHeight:1.25,text:originalText.replace('p-','p-\n'),originalText,autoResize:false});
 const box={...shape,id:'batch-box',x:500,height:180,boundElements:[{id:'batch-bound',type:'text'}]};
 const bound={...note('batch-bound',510,'信号 p-STAT1 入核'),containerId:box.id,width:200,y:40};
 const document={type:'excalidraw',version:2,elements:[note('batch-a',0,'前缀 p-IFNGR1 细胞'),note('batch-b',250,'信号 IFN-γ 进入细胞'),box,bound],files:{},customData:{review:'preserve batch source metadata'}};
 const open=async(title,doc)=>{
  const figure=await call('POST','/test/visualization',{action:'create',project_id:project,title,document:doc});
  await page.getByRole('button',{name:'刷新画布列表',exact:true}).click();await page.getByRole('button',{name:title,exact:true}).click();await page.locator('[data-canvas-surface] .excalidraw:visible').waitFor();await reselect();
  return `/api/v1/visualization/projects/${project}/figures/${figure.content.id}`;
 };
 const path=await open('Batch text reflow',document),panel=edit.getByRole('region',{name:'批量文字重排',exact:true});await panel.waitFor();
 assert.match(await panel.innerText(),/已选择 3 段文字/);assert.equal(await edit.getByRole('textbox',{name:'文字原文',exact:true}).count(),0);
 const head=await call('GET',path);await persist('batch-before.json',head);
 const event=page.waitForRequest(r=>r.method()==='POST'&&new URL(r.url()).pathname.endsWith('/proposals'));
 await panel.getByRole('button',{name:'预览批量文字重排',exact:true}).click();const payload=(await event).postDataJSON();
 assert.deepEqual(payload.operations,head.scene.document.elements.filter(e=>e.type==='text').map(e=>({op:'set_wrapped_text',id:e.id,text:e.originalText})));
 await dialog.getByRole('region',{name:'候选画布'}).locator('.excalidraw').waitFor();assert.deepEqual(await call('GET',path),head);
 const candidates=await call('GET',`${path}/proposals`);assert.equal(candidates.length,1);const proposal=await call('GET',`${path}/proposals/${candidates[0].id}`),restored=structuredClone(proposal.document);
 for(const prior of head.scene.document.elements){
  const actual=find(proposal.document,prior.id);if(prior.type!=='text'){assert.deepEqual(actual,prior);continue;}
  assert.equal(actual.originalText,prior.originalText);for(const field of ['fontSize','fontFamily','lineHeight','containerId','customData'])assert.deepEqual(actual[field],prior[field]);if(!prior.containerId)assert.equal(actual.width,prior.width);
  for(const field of ['x','y','width','height','text','version','versionNonce','updated']){if(Object.hasOwn(prior,field))find(restored,prior.id)[field]=prior[field];else delete find(restored,prior.id)[field];}
 }
 assert.deepEqual(restored,head.scene.document,'only selected text layout/stamps change');assert.ok(find(proposal.document,'batch-a').text.includes('p-IFNGR1'));assert.ok(find(proposal.document,'batch-b').text.includes('IFN-γ'));
 await page.screenshot({path:join(directory,'batch-preview.png'),animations:'disabled'});await dialog.getByRole('button',{name:'采用提案',exact:true}).click();await dialog.waitFor({state:'hidden'});assert.deepEqual((await call('GET',path)).scene.document,proposal.document);
 const download=page.waitForEvent('download');await page.getByRole('button',{name:'导出 源文件',exact:true}).click();const exported=JSON.parse(await readFile(await(await download).path(),'utf8'));assert.deepEqual(exported.files,proposal.document.files);
 for(const element of proposal.document.elements){const actual=find(exported,element.id);for(const key of ['text','originalText','fontFamily','fontSize','width','height','containerId','customData'])assert.deepEqual(actual[key],element[key]);}
 await persist('batch-proposal.json',proposal);await persist('batch-source.json',exported);
 const invalid=structuredClone(document);invalid.elements[1].angle=0.2;const invalidPath=await open('Invalid batch text',invalid);
 await edit.getByRole('alert').filter({hasText:'未旋转'}).waitFor();assert.equal(await edit.getByRole('button',{name:'预览批量文字重排',exact:true}).count(),0);assert.deepEqual(await call('GET',`${invalidPath}/proposals`),[]);await page.keyboard.press('Escape');
 const excessive={...structuredClone(document),elements:Array.from({length:21},(_,index)=>note(`limit-${index}`,index*200,`Label ${index}`))};
 const limitPath=await open('Too many text labels',excessive);await edit.getByRole('alert').filter({hasText:'1–20'}).waitFor();assert.equal(await edit.getByRole('button',{name:'预览批量文字重排',exact:true}).count(),0);assert.deepEqual(await call('GET',`${limitPath}/proposals`),[]);await page.keyboard.press('Escape');
 // A late native validation failure must leave no partially reflowed candidate.
 const unsupported=structuredClone(document);unsupported.elements[1].text='Unsupported 😀';unsupported.elements[1].originalText='Unsupported 😀';
 const unsupportedPath=await open('Atomic reflow validation',unsupported);await panel.waitFor();const unsupportedHead=await call('GET',unsupportedPath);
 await panel.getByRole('button',{name:'预览批量文字重排',exact:true}).click();await edit.getByRole('alert').filter({hasText:'pinned scientific font'}).waitFor();assert.deepEqual(await call('GET',unsupportedPath),unsupportedHead);assert.deepEqual(await call('GET',`${unsupportedPath}/proposals`),[]);await page.keyboard.press('Escape');
 // Another writer advances the server while the review is open.
 const stalePath=await open('Stale batch text review',document);await panel.waitFor();const staleHead=await call('GET',stalePath);
 await call('PUT',stalePath,{expected_revision:staleHead.metadata.revision,title:'Externally saved figure',document:staleHead.scene.document});const latest=await call('GET',stalePath);
 await panel.getByRole('button',{name:'预览批量文字重排',exact:true}).click();await edit.getByRole('alert').waitFor();assert.deepEqual(await call('GET',stalePath),latest);assert.deepEqual(await call('GET',`${stalePath}/proposals`),[]);await persist('batch-stale-head.json',latest);await page.keyboard.press('Escape');
}
