import {parseContextMessage,serializeContext} from '../src/features/context/context-model.ts';
import {readPathPreparedContext} from './helpers/canvas-prepared-context.mjs';
// Real BioArt catalogue SVG through GUI insertion, path editing, adoption and export; no model invocation.
import assert from 'node:assert/strict';
import {chromium} from 'playwright';
import {spawn} from 'node:child_process';
import {mkdtemp,readFile,writeFile} from 'node:fs/promises';
import {existsSync} from 'node:fs';
import {tmpdir} from 'node:os';
import {join} from 'node:path';
import {fileURLToPath} from 'node:url';
const binary=process.env.VISUALIZATION_NATIVE_BIN||fileURLToPath(new URL('../../../target/debug/examples/visualization_native',import.meta.url));
const strokeWidthMode=process.env.VISUALIZATION_STROKE_WIDTH==='1';
const pathContext=process.env.VISUALIZATION_PATH_CONTEXT==='1';const candidateDraft=process.env.VISUALIZATION_PART_CANDIDATE_DRAFT==='1'||pathContext;const agentDraft=process.env.VISUALIZATION_PART_AGENT_DRAFT==='1'||candidateDraft;let autoMessages=0,explicitMessages=0;const sentMessages=[];let staleParts=false,staleCandidate=false;
const directory=await mkdtemp(join(tmpdir(),'autonomics-bioart-svg-')),origin='http://127.0.0.1:1458';
const native=spawn(binary,[join(directory,'figures.sqlite')],{stdio:['ignore','pipe','pipe']}),vite=spawn(process.execPath,['node_modules/vite/bin/vite.js','--host','127.0.0.1','--port','1458','--strictPort'],{stdio:['ignore','pipe','pipe']});
let output='',viteOutput='',browser,page;const diagnostics={requests:[],console:[]};native.stdout.on('data',b=>output+=b);native.stderr.on('data',b=>output+=b);vite.stdout.on('data',b=>viteOutput+=b);vite.stderr.on('data',b=>viteOutput+=b);
const persist=(name,value)=>writeFile(join(directory,name),JSON.stringify(value,null,2),{mode:0o600});
const until=async(fn,label)=>{const deadline=Date.now()+60000;while(!await fn()){assert.ok(Date.now()<deadline,label);await new Promise(r=>setTimeout(r,50));}};
try {
 await until(()=>{assert.equal(native.exitCode,null,output);return output.includes('VISUALIZATION_NATIVE_ADDR=');},'native readiness');await until(()=>{assert.equal(vite.exitCode,null,viteOutput);return viteOutput.includes(origin);},'Vite readiness');
 const backend=`http://${output.match(/VISUALIZATION_NATIVE_ADDR=([^\s]+)/)[1]}`;
 const call=async(method,path,body)=>{const r=await fetch(backend+path,{method,headers:{'content-type':'application/json'},...(body===undefined?{}:{body:JSON.stringify(body)}),signal:AbortSignal.timeout(30000)});const v=await r.json();assert.ok(r.ok,JSON.stringify(v));return v;};
 browser=await chromium.launch({executablePath:process.env.PDF_TEST_CHROMIUM||(existsSync('/usr/bin/chromium')?'/usr/bin/chromium':undefined),args:['--no-sandbox']});page=await browser.newPage({viewport:{width:1600,height:1000}});const errors=[];page.on('pageerror',e=>errors.push(e.message));page.on('requestfailed',r=>{if(diagnostics.requests.length<100)diagnostics.requests.push({url:r.url(),error:r.failure()?.errorText});});page.on('console',m=>{if(m.type()==='error'&&diagnostics.console.length<100)diagnostics.console.push(m.text().slice(0,1000));});
 await page.route('**/api/session',r=>r.fulfill({json:{}}));
 await page.route('**/api/v1/**',async route=>{const url=new URL(route.request().url()),p=url.pathname.replace('/api/v1','');if(p.startsWith('/visualization/')){if(staleCandidate&&p.endsWith('/vector-parts')&&url.searchParams.has('proposal_id')){staleCandidate=false;const response=await route.fetch({url:backend+url.pathname+url.search}),value=await response.json();value.document_sha256='stale-candidate-hash';return route.fulfill({json:value});}if(staleParts&&p.endsWith('/vector-parts')&&route.request().method()==='GET'){staleParts=false;const response=await route.fetch({url:backend+url.pathname+url.search}),value=await response.json();value.sha256='stale-file-hash';return route.fulfill({json:value});}return route.fulfill({response:await route.fetch({url:backend+url.pathname+url.search})});}if(p==='/events')return new Promise(()=>{});if(p==='/state')return route.fulfill({json:{last_seq:0,agents:agentDraft?[{path:'/root/alpha',agent_id:'a1',status:'Idle'}]:[],contexts:agentDraft?{'/root/alpha':{id:'s1',title:'Path draft',created_at:1,last_active:1}}:{}}});if(p.endsWith('/history'))return route.fulfill({json:{messages:[]}});if(p.endsWith('/messages')&&route.request().method()==='POST'){autoMessages++;sentMessages.push(route.request().postDataJSON());return route.fulfill({json:{}});}if(p==='/profiles')return route.fulfill({json:[]});if(p==='/model-config')return route.fulfill({json:{models:[],providers:[]}});if(p==='/settings')return route.fulfill({json:{}});if(p==='/storage/agents')return route.fulfill({json:[]});if(p==='/gateway/status')return route.fulfill({json:{running:true}});return route.fulfill({status:404,json:{error:`unrelated fixture ${p}`}});});
 await page.goto(origin,{waitUntil:'domcontentloaded',timeout:60000});await page.getByRole('navigation',{name:'工作台活动栏'}).getByRole('button',{name:'画布',exact:true}).click();await page.getByRole('textbox',{name:'新项目名称'}).fill('Illustration fit');await page.getByRole('button',{name:'创建项目',exact:true}).click();await page.getByRole('button',{name:'新建画布',exact:true}).click();await page.locator('[data-canvas-surface] .excalidraw:visible').waitFor();
 const project=await page.getByRole('combobox',{name:'可视化项目'}).inputValue(),figure=(await call('GET',`/api/v1/visualization/projects/${project}/figures`))[0].id,path=`/api/v1/visualization/projects/${project}/figures/${figure}`;

 async function checkAutomaticContext(expected) {
  const target=page.getByRole('combobox',{name:'Agent 上下文目标'});await target.selectOption({value:'$follow'});
  const details=page.locator('details').filter({has:page.getByText('查看发送时附加的状态',{exact:true})});if(await details.getAttribute('open')===null)await details.locator('summary').click();
  let shared;await until(async()=>{try{const value=JSON.parse(await details.locator('pre').textContent());shared=value.inspectedSvgPath?JSON.parse(value.inspectedSvgPath):null;return shared?.part_id===expected.selected_path.part_id&&shared.panel_visible===false;}catch{return false;}},'live shared SVG path context');
  assert.equal(shared.figure_id,figure);assert.equal(shared.project_id,project);assert.equal(shared.sha256,expected.sha256);assert.equal(shared.expected_revision,expected.expected_revision);assert.equal(shared.proposal_id,expected.proposal_id);assert.equal(shared.document_sha256,expected.document_sha256);
  const actions=shared.follow_up_inputs;assert.ok(actions);for(const args of Object.values(actions))assert.equal(args.markup_ids,undefined);
  const inspected=await call('POST','/test/bioicons',actions.parts),metadata=JSON.parse(inspected.content.find(b=>b.type==='text').text);assert.equal(metadata.sha256,shared.sha256);assert.equal(metadata.proposal_id,shared.proposal_id);assert.equal(metadata.document_sha256,shared.document_sha256);
  const chosen=metadata.parts.find(p=>p.part_id===shared.part_id);assert.ok(chosen);if(chosen.geometry_supported){const geometry=await call('POST','/test/bioicons',actions.geometry);assert.ok(geometry.content.some(b=>b.type==='image'));}
  const composer=page.getByPlaceholder('输入消息…'),previous=await composer.inputValue();assert.equal(autoMessages,explicitMessages);await composer.fill('仅查看我刚才检查的路径，不执行修改。');explicitMessages++;await page.getByRole('button',{name:'发送',exact:true}).click();await until(()=>autoMessages===explicitMessages,'explicit mock context send');
  const attached=parseContextMessage(sentMessages.at(-1).text).resources,resource=attached.find(r=>r.kind==='editor'&&!r.id.startsWith('canvas-request:')),snapshot=JSON.parse(resource.content),sentPath=JSON.parse(snapshot.inspectedSvgPath);assert.deepEqual(sentPath,shared);assert.ok(!resource.content.includes('data:image'));await persist(`shared-path-message-${explicitMessages}.json`,{snapshot,auto_messages:autoMessages-explicitMessages,explicit_messages:explicitMessages});await composer.fill(previous+serializeContext(attached.filter(r=>r.id.startsWith('canvas-request:'))));
 }

 let initial;
 const savedScene=process.env.VISUALIZATION_BIOART_SAVED_SCENE;
 if(savedScene){
  const input=JSON.parse(await readFile(savedScene,'utf8')),document=input.scene?.document??input.document??input,head=await call('GET',path);
  await call('PUT',path,{expected_revision:head.metadata.revision,title:'Saved BioArt SVG width',document});page.once('dialog',d=>d.accept());await page.getByRole('button',{name:'重新加载服务端版本',exact:true}).click();initial=await call('GET',path);
  const surface=page.locator('[data-canvas-surface] .excalidraw:visible');await surface.waitFor();const box=await surface.boundingBox(),elements=document.elements.filter(e=>!e.isDeleted),image=elements.filter(e=>e.type==='image').at(-1),left=Math.min(...elements.map(e=>e.x)),right=Math.max(...elements.map(e=>e.x+e.width)),top=Math.min(...elements.map(e=>e.y)),bottom=Math.max(...elements.map(e=>e.y+e.height));
  await page.mouse.click(box.x+box.width/2+image.x+image.width/2-(left+right)/2,box.y+box.height/2+image.y+image.height/2-(top+bottom)/2);
 }else{
 await page.getByRole('button',{name:'BioArt 素材',exact:true}).click();
 const panel=page.getByRole('complementary',{name:'BioArt 素材',exact:true});
 await panel.getByRole('combobox',{name:'BioArt 插入格式'}).selectOption('svg');
 await panel.getByRole('textbox',{name:'BioArt 搜索词'}).fill('Mitochondria');
 await panel.getByRole('button',{name:'搜索',exact:true}).click();
 const article=panel.locator('article').filter({has:page.getByText('Mitochondria',{exact:true})}).first();
 await article.getByRole('button',{name:'预览图片',exact:true}).click();
 await article.getByRole('img',{name:'Mitochondria',exact:true}).waitFor({timeout:60000});
 await article.getByRole('button',{name:'插入图片',exact:true}).click();
 await article.getByRole('button',{name:'插入图片',exact:true}).click();
 await page.getByRole('button',{name:'保存画布',exact:true}).click();
 await until(async()=>{initial=await call('GET',path);return initial.scene.document.elements.filter(e=>e.type==='image').length===2;},'SVG inserted and saved');
 }
 const doc=initial.scene.document,image=doc.elements.filter(e=>e.type==='image').at(-1);
 assert.equal(doc.files[image.fileId].mimeType,'image/svg+xml');
 assert.equal(image.customData.source,'NIH BioArt');assert.equal(image.customData.asset.title,'Mitochondria');
 assert.ok(image.customData.asset.creators.length&&image.customData.asset.licenses.length);
 assert.ok(image.customData.source_sha256);assert.equal(image.customData.file.format.toLowerCase(),'svg');
 await persist('inserted.json',initial);
 await page.getByRole('button',{name:'素材编辑',exact:true}).click();
 const editor=page.getByRole('dialog',{name:'矢量素材编辑',exact:true});
 await editor.getByRole('button',{name:'读取内部路径',exact:true}).click();
 await editor.getByRole('combobox',{name:'素材路径',exact:true}).waitFor();
 const parts=await call('GET',`${path}/vector-parts?element_id=${image.id}&expected_revision=${initial.metadata.revision}`);
 const selected=strokeWidthMode?(parts.parts.find(p=>p.stroke_width>0)||parts.parts[0]):parts.parts.find(p=>p.translation_supported);assert.ok(selected);
 await editor.getByRole('combobox',{name:'素材路径',exact:true}).selectOption({value:String(parts.parts.findIndex(p=>p.part_id===selected.part_id))});
 assert.equal(parts.render_contribution.status,'completed');
 const contribution=editor.locator('[aria-label="路径渲染贡献"]');await contribution.waitFor();
 assert.ok((await contribution.textContent()).includes(`隐藏此路径会改变 ${selected.changed_preview_pixels_when_hidden} 个像素`));
 assert.ok((await contribution.textContent()).includes('这不是路径面积'));
 if(agentDraft){
  const draftPanel=editor.getByRole('region',{name:'交给 Agent 修改路径'});
  await draftPanel.getByRole('combobox',{name:'路径修改 Agent'}).selectOption('/root/alpha');
  await draftPanel.getByRole('textbox',{name:'路径修改要求'}).fill('只把当前路径改成蓝色，保留其他路径。');
  staleParts=true;await draftPanel.getByRole('button',{name:'准备路径修改草稿',exact:true}).click();
  await editor.getByRole('alert').filter({hasText:'素材版本或路径已变化'}).waitFor();
  assert.deepEqual(await call('GET',path),initial);assert.equal(autoMessages,explicitMessages);
  await call('PUT',path,{expected_revision:initial.metadata.revision,title:initial.metadata.title,document:initial.scene.document});
  const changedHead=await call('GET',path);assert.equal(changedHead.metadata.revision,initial.metadata.revision+1);
  const rejected=page.waitForResponse(r=>r.url().includes('/vector-parts?')&&r.status()===409);
  await draftPanel.getByRole('button',{name:'准备路径修改草稿',exact:true}).click();await rejected;await editor.getByRole('alert').waitFor();assert.equal(autoMessages,explicitMessages);assert.deepEqual(await call('GET',path),changedHead);
  await editor.getByRole('button',{name:'关闭素材编辑',exact:true}).click();await page.getByRole('button',{name:'重新加载服务端版本',exact:true}).click();initial=await call('GET',path);
  const surface=page.locator('[data-canvas-surface] .excalidraw:visible');await surface.waitFor();const box=await surface.boundingBox(),elements=initial.scene.document.elements;
  const left=Math.min(...elements.map(e=>e.x)),right=Math.max(...elements.map(e=>e.x+e.width)),top=Math.min(...elements.map(e=>e.y)),bottom=Math.max(...elements.map(e=>e.y+e.height));
  await page.mouse.click(box.x+box.width/2+image.x+image.width/2-(left+right)/2,box.y+box.height/2+image.y+image.height/2-(top+bottom)/2);
  await page.getByRole('button',{name:'素材编辑',exact:true}).click();await editor.getByRole('button',{name:'读取内部路径',exact:true}).click();await editor.getByRole('combobox',{name:'素材路径',exact:true}).selectOption({value:String(parts.parts.findIndex(p=>p.part_id===selected.part_id))});
  await draftPanel.getByRole('combobox',{name:'路径修改 Agent'}).selectOption('/root/alpha');await draftPanel.getByRole('textbox',{name:'路径修改要求'}).fill('只把当前路径改成蓝色，保留其他路径。');
  await draftPanel.getByRole('button',{name:'准备路径修改草稿',exact:true}).click();
  await editor.waitFor({state:'hidden'});const composer=page.getByPlaceholder('输入消息…');await composer.waitFor();const draft=await composer.inputValue(),context=await readPathPreparedContext(page);
  assert.equal(draft,'只把当前路径改成蓝色，保留其他路径。');await composer.fill('Edited path request: preserve the snapshot');assert.deepEqual(await readPathPreparedContext(page),context);await composer.fill(draft);assert.equal(context.project_id,project);assert.equal(context.figure_id,figure);assert.equal(context.expected_revision,initial.metadata.revision);assert.equal(context.element_id,image.id);assert.equal(context.file_id,image.fileId);assert.equal(context.sha256,parts.sha256);assert.equal(context.selected_path.part_id,selected.part_id);assert.deepEqual(context.selected_path.bounds,selected.bounds);assert.equal(context.selected_path.stroke_width,selected.stroke_width);assert.deepEqual(context.follow_up_inputs.read.element_ids,[image.id]);assert.equal(context.follow_up_inputs.geometry.part_id,selected.part_id);assert.equal(context.render_contribution.status,'completed');assert.equal(context.preview_available,true);assert.equal(context.follow_up_inputs.read_preview.expected_revision,initial.metadata.revision);assert.equal(context.follow_up_inputs.read_image.element_id,image.id);const published=await call('GET',`${path}/revisions/${initial.metadata.revision}/preview`);assert.ok(published.dataURL.startsWith('data:image/png;base64,'));const toolPreview=await call('POST','/test/visualization',context.follow_up_inputs.read_preview);assert.ok(toolPreview.content.some(block=>block.type==='image'));assert.ok(!draft.includes('data:image'));assert.equal(autoMessages,explicitMessages);assert.deepEqual(await call('GET',path),initial);assert.equal((await call('GET',`${path}/proposals`)).length,0);
  await persist('path-agent-draft.json',{draft,context,auto_messages:autoMessages-explicitMessages,explicit_messages:explicitMessages,stale_hash_rejected:true,stale_revision_rejected:true});if(pathContext)await checkAutomaticContext(context);
  await page.getByRole('tab',{name:initial.metadata.title,exact:true}).click();await page.getByRole('button',{name:'素材编辑',exact:true}).click();await editor.getByRole('button',{name:'读取内部路径',exact:true}).click();await editor.getByRole('combobox',{name:'素材路径',exact:true}).selectOption({value:String(parts.parts.findIndex(p=>p.part_id===selected.part_id))});
 }
 let requestedStrokeWidth;
 if(strokeWidthMode){
  assert.equal(typeof selected.stroke_width,'number');requestedStrokeWidth=Math.max(1,selected.stroke_width*2);
  await editor.getByRole('checkbox',{name:'修改路径描边宽度',exact:true}).check();const width=editor.getByRole('spinbutton',{name:'路径描边宽度',exact:true});
  for(const invalid of ['0','-1','1001']){await width.fill(invalid);assert.equal(await editor.getByRole('button',{name:'预览路径修改提案',exact:true}).isEnabled(),false);assert.equal((await call('GET',`${path}/proposals`)).length,0);}
  await width.fill(String(requestedStrokeWidth));
  if(selected.stroke_width===0){await editor.getByRole('checkbox',{name:'修改路径描边',exact:true}).check();await editor.getByRole('checkbox',{name:'路径描边设为空',exact:true}).uncheck();await editor.getByLabel('路径描边颜色',{exact:true}).fill('#000000');}
 }else{
 await editor.getByRole('checkbox',{name:'平移路径',exact:true}).check();
 await editor.getByLabel('路径平移 X',{exact:true}).fill('3');
 await editor.getByLabel('路径平移 Y',{exact:true}).fill('2');
 }
 await editor.getByRole('button',{name:'预览路径修改提案',exact:true}).click();
 const preview=page.getByRole('dialog',{name:'Agent 修改预览',exact:true});
 await preview.getByRole('region',{name:'候选画布'}).locator('.excalidraw').waitFor();
 const candidates=(await call('GET',`${path}/proposals`)).filter(p=>p.status==='pending');assert.equal(candidates.length,1);
 let candidate=candidates[0];const updated=candidate.document.elements.find(e=>e.id===image.id);
 assert.equal((await call('GET',path)).metadata.revision,initial.metadata.revision);
 for(const key of ['source','asset','file','source_sha256'])assert.deepEqual(updated.customData[key],image.customData[key]);
 for(const key of ['id','x','y','width','height','scale','crop'])assert.deepEqual(updated[key],image[key]);
 assert.notEqual(updated.fileId,image.fileId);assert.equal(updated.customData.modifications[0].kind,strokeWidthMode?'path_style':'path_edit');
 for(const element of doc.elements.filter(e=>e.id!==image.id))assert.deepEqual(candidate.document.elements.find(e=>e.id===element.id),element);
 for(const [id,file] of Object.entries(doc.files))assert.deepEqual(candidate.document.files[id],file);
 if(candidateDraft){
  const priorCandidate=candidate,review=await call('POST',`${path}/markups`,{expected_revision:initial.metadata.revision,note:'Refine chosen SVG path',strokes:[{kind:'draw',points:[[0,0],[20,20]]}],dataURL:'data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVQIHWP4z8DwHwAFgAI/ScLbtAAAAABJRU5ErkJggg=='});
  candidate=await call('POST',`${path}/proposals/${priorCandidate.id}/revisions`,{expected_revision:initial.metadata.revision,parent_document_sha256:priorCandidate.document_sha256,summary:'Linked path review',operations:[],markup_ids:[review.id]});
  await preview.getByRole('button',{name:'关闭预览',exact:true}).click();await page.getByRole('button',{name:'查看 Agent 提案',exact:true}).click();await preview.getByRole('button',{name:`${candidate.summary} · v${initial.metadata.revision} · 修订`,exact:true}).click();await preview.getByRole('region',{name:'候选画布'}).locator('.excalidraw').waitFor();
  await preview.getByRole('combobox',{name:'候选对象',exact:true}).selectOption(image.id);
  const section=preview.locator('details[aria-label="候选素材路径修改"]');await section.locator('summary').click();await section.getByRole('button',{name:'读取候选内部路径',exact:true}).click();await section.getByRole('combobox',{name:'候选素材路径',exact:true}).waitFor();
  const inspected=await call('GET',`${path}/vector-parts?element_id=${image.id}&expected_revision=${initial.metadata.revision}&proposal_id=${candidate.id}&document_sha256=${candidate.document_sha256}`);assert.notEqual(inspected.sha256,parts.sha256);const chosen=inspected.parts.at(-1);await section.getByRole('combobox',{name:'候选素材路径',exact:true}).selectOption({value:String(chosen.index-1)});
  const panel=section.getByRole('region',{name:'交给 Agent 修改路径'});await persist('candidate-path-selection.json',{chosen,options:await section.getByRole('combobox',{name:'候选素材路径'}).locator('option').evaluateAll(options=>options.map(o=>({value:o.value,text:o.textContent})))});await until(async()=>await panel.getAttribute('data-svg-part-id')===chosen.part_id,'candidate path selection settles');await panel.getByRole('combobox',{name:'路径修改 Agent'}).selectOption('/root/alpha');await panel.getByRole('textbox',{name:'路径修改要求'}).fill('仅将候选中选定路径填充改为蓝色，保留本候选前序修改。');
  await section.getByRole('checkbox',{name:'Refine chosen SVG path',exact:true}).check();staleCandidate=true;await panel.getByRole('button',{name:'准备路径修改草稿',exact:true}).click();await section.getByRole('alert').filter({hasText:'候选身份已变化'}).waitFor();assert.equal(autoMessages,explicitMessages);assert.deepEqual(await call('GET',`${path}/proposals/${candidate.id}`),candidate);
  await panel.getByRole('button',{name:'准备路径修改草稿',exact:true}).click();await preview.waitFor({state:'hidden'});const composer=page.getByPlaceholder('输入消息…');await composer.waitFor();const draft=await composer.inputValue();const context=await readPathPreparedContext(page);
  assert.equal(context.scope,'pending_candidate_svg_path');assert.equal(context.proposal_id,candidate.id);assert.equal(context.document_sha256,candidate.document_sha256);assert.equal(context.sha256,inspected.sha256);assert.equal(context.selected_path.part_id,chosen.part_id);assert.equal(context.follow_up_inputs.parts.proposal_id,candidate.id);assert.equal(context.follow_up_inputs.parts.document_sha256,candidate.document_sha256);assert.deepEqual(context.follow_up_inputs.read_proposal.element_ids,[image.id]);assert.equal(context.follow_up_inputs.read_proposal_preview.proposal_id,candidate.id);assert.equal(context.follow_up_inputs.read,undefined);assert.equal(context.follow_up_inputs.read_image,undefined);assert.deepEqual(context.markup_ids,[review.id]);assert.equal((await call('GET',`${path}/markups`)).find(m=>m.id===review.id).status,'open');assert.ok(draft.includes('只把当前路径改成蓝色'),'existing formal draft is preserved');assert.equal(autoMessages,explicitMessages);assert.deepEqual(await call('GET',path),initial);assert.deepEqual((await call('GET',`${path}/proposals`)).map(p=>p.id).sort(),[candidate.id,priorCandidate.id].sort());
  await persist('candidate-path-draft.json',{context,auto_messages:autoMessages-explicitMessages,explicit_messages:explicitMessages,stale_candidate_hash_rejected:true,linked_review_id:review.id,parent_unchanged:await call('GET',`${path}/proposals/${candidate.id}`)});if(pathContext)await checkAutomaticContext(context);
  await page.getByRole('tab',{name:initial.metadata.title,exact:true}).click();await page.getByRole('button',{name:'查看 Agent 提案',exact:true}).click();await preview.getByRole('button',{name:`${candidate.summary} · v${initial.metadata.revision}${candidate.parent_proposal_id?' · 修订':''}`,exact:true}).click();await preview.getByRole('region',{name:'候选画布'}).locator('.excalidraw').waitFor();
  if(pathContext){
   await preview.getByRole('combobox',{name:'候选对象',exact:true}).selectOption({value:image.id});const inspectPanel=preview.locator('details[aria-label="候选素材路径修改"]');await inspectPanel.locator('summary').click();await inspectPanel.getByRole('button',{name:'读取候选内部路径',exact:true}).click();const pick=inspectPanel.getByRole('combobox',{name:'候选素材路径',exact:true});await pick.waitFor();await pick.selectOption({value:'0'});const independent=inspected.parts[0];assert.notEqual(independent.part_id,chosen.part_id);await until(async()=>await inspectPanel.getByRole('region',{name:'交给 Agent 修改路径'}).getAttribute('data-svg-part-id')===independent.part_id,'independently inspected path');
   await preview.getByRole('button',{name:'关闭预览',exact:true}).click();await page.getByRole('tab',{name:'alpha',exact:true}).click();await checkAutomaticContext({...context,selected_path:{part_id:independent.part_id}});assert.deepEqual(await call('GET',path),initial);await persist('independent-path-context.json',{entry:'inspect a different path, close manually, send ordinary mock chat without preparing path draft',part_id:independent.part_id,explicit_messages:explicitMessages,auto_messages:autoMessages-explicitMessages});
   await page.getByRole('tab',{name:initial.metadata.title,exact:true}).click();await page.getByRole('button',{name:'查看 Agent 提案',exact:true}).click();await preview.getByRole('button',{name:`${candidate.summary} · v${initial.metadata.revision}${candidate.parent_proposal_id?' · 修订':''}`,exact:true}).click();await preview.getByRole('region',{name:'候选画布'}).locator('.excalidraw').waitFor();
  }
 }
 await preview.getByRole('button',{name:'采用提案',exact:true}).click();await preview.waitFor({state:'hidden'});
 const adopted=await call('GET',path);assert.equal(adopted.metadata.revision,initial.metadata.revision+1);
 const afterParts=await call('GET',`${path}/vector-parts?element_id=${image.id}&expected_revision=${adopted.metadata.revision}`);
 assert.equal(afterParts.parts.length,parts.parts.length);
 if(strokeWidthMode){for(let i=0;i<parts.parts.length;i++){const before=parts.parts[i],after=afterParts.parts[i];assert.equal(after.point_count,before.point_count);assert.equal(after.fill,before.fill);assert.equal(after.opacity,before.opacity);if(before.index===selected.index)assert.ok(Math.abs(after.stroke_width-requestedStrokeWidth)<1e-5);else{assert.equal(after.stroke_width,before.stroke_width);assert.deepEqual(after.bounds,before.bounds);assert.equal(after.stroke,before.stroke);}}}else{for(let i=0;i<parts.parts.length;i++)for(let axis=0;axis<4;axis++)assert.ok(Math.abs(afterParts.parts[i].bounds[axis]-parts.parts[i].bounds[axis]-(parts.parts[i].index===selected.index&&axis<2?[3,2][axis]:0))<.001);}
 const download=async name=>{const pending=page.waitForEvent('download');await page.getByRole('button',{name,exact:true}).click();return readFile(await(await pending).path());};
 await page.getByRole('button',{name:'重新加载服务端版本',exact:true}).click();
 const exported=JSON.parse((await download('导出 源文件')).toString());
 const restored=exported.elements.find(e=>e.id===image.id);
 for(const key of ['id','fileId','x','y','width','height','scale','crop','customData'])assert.deepEqual(restored[key],updated[key]);
 assert.deepEqual(exported.files,candidate.document.files);
 const manifest=JSON.parse((await download('导出素材来源')).toString()),entry=manifest.assets.find(a=>a.element_id===image.id);
 assert.deepEqual(entry.authors,image.customData.asset.creators);assert.deepEqual(entry.licenses,image.customData.asset.licenses);assert.equal(entry.hash_matches_recorded,true);if(strokeWidthMode)assert.equal(entry.modifications[0].edits[0].stroke_width,requestedStrokeWidth);
 const svg=(await download('导出 SVG')).toString();assert.ok(svg.includes('autonomics-asset-sources'));
 await writeFile(join(directory,'final.svg'),svg);await writeFile(join(directory,'final.png'),await download('导出 PNG'));
 await persist('source.json',exported);await persist('source-manifest.json',manifest);await persist('parts-before.json',parts);await persist('parts-after.json',afterParts);
 if(pathContext)await page.locator('[aria-label="SVG 路径上下文"]').waitFor({state:'hidden'});
 assert.deepEqual(errors,[]);await persist('result.json',{passed:true,model_used:false,live_catalog:!savedScene,saved_scene_replay:Boolean(savedScene),stroke_width_gui_verified:strokeWidthMode,requested_stroke_width:requestedStrokeWidth,path_agent_draft_verified:agentDraft,candidate_path_draft_verified:candidateDraft,shared_path_context_verified:pathContext,asset_id:image.customData.asset.id,scope:strokeWidthMode?'GUI saved or downloaded BioArt SVG path stroke-width edit; invalid values disabled, other paths and source retained; adoption/reload/source/SVG/PNG exports; not scientific validation':'GUI BioArt SVG search, preview, insert, save, path translation proposal, adoption, reload, native and source exports; catalogue provenance retained'});
 console.log(`bioart-svg: passed; artifacts=${directory}`);
} catch(error){await persist('failure.json',{error:String(error),stack:error.stack,diagnostics,native_output:output,vite_output:viteOutput});if(page)await page.screenshot({path:join(directory,'failure.png'),animations:'disabled'}).catch(()=>{});console.error(`bioart-svg: failed; artifacts=${directory}`);throw error;}finally{if(browser)await browser.close();for(const p of [vite,native]){if(p.exitCode===null&&p.signalCode===null){p.kill();await new Promise(resolve=>p.once('exit',resolve));}}}
