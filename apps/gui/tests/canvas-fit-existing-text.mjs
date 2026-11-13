import {checkHumanBatchReflow} from './helpers/canvas-human-text-batch.mjs';
// Fit exactly one text box in an explicitly supplied saved scene; no model invocation.
import assert from 'node:assert/strict';
import {chromium} from 'playwright';
import {spawn} from 'node:child_process';
import {mkdtemp,readFile,writeFile} from 'node:fs/promises';
import {existsSync} from 'node:fs';
import {tmpdir} from 'node:os';
import {join} from 'node:path';
import {fileURLToPath} from 'node:url';
const binary=process.env.VISUALIZATION_NATIVE_BIN||fileURLToPath(new URL('../../../target/debug/examples/visualization_native',import.meta.url));
const directory=await mkdtemp(join(tmpdir(),'autonomics-existing-text-')),origin='http://127.0.0.1:1459';
const native=spawn(binary,[join(directory,'figures.sqlite')],{stdio:['ignore','pipe','pipe']}),vite=spawn(process.execPath,['node_modules/vite/bin/vite.js','--host','127.0.0.1','--port','1459','--strictPort'],{stdio:['ignore','pipe','pipe']});
let output='',viteOutput='',browser,page;let sent=0;const diagnostics={requests:[],console:[]};native.stdout.on('data',b=>output+=b);native.stderr.on('data',b=>output+=b);vite.stdout.on('data',b=>viteOutput+=b);vite.stderr.on('data',b=>viteOutput+=b);
const persist=(name,value)=>writeFile(join(directory,name),JSON.stringify(value,null,2),{mode:0o600});
const until=async(fn,label)=>{const deadline=Date.now()+60000;while(!await fn()){assert.ok(Date.now()<deadline,label);await new Promise(r=>setTimeout(r,50));}};
try {
 await until(()=>{assert.equal(native.exitCode,null,output);return output.includes('VISUALIZATION_NATIVE_ADDR=');},'native readiness');await until(()=>{assert.equal(vite.exitCode,null,viteOutput);return viteOutput.includes(origin);},'Vite readiness');
 const backend=`http://${output.match(/VISUALIZATION_NATIVE_ADDR=([^\s]+)/)[1]}`;
 const call=async(method,path,body)=>{const r=await fetch(backend+path,{method,headers:{'content-type':'application/json'},...(body===undefined?{}:{body:JSON.stringify(body)}),signal:AbortSignal.timeout(30000)});const v=await r.json();assert.ok(r.ok,JSON.stringify(v));return v;};
 browser=await chromium.launch({executablePath:process.env.PDF_TEST_CHROMIUM||(existsSync('/usr/bin/chromium')?'/usr/bin/chromium':undefined),args:['--no-sandbox']});page=await browser.newPage({viewport:{width:1600,height:1000}});const errors=[];page.on('pageerror',e=>errors.push(e.message));page.on('requestfailed',r=>{if(diagnostics.requests.length<100)diagnostics.requests.push({url:r.url(),error:r.failure()?.errorText});});page.on('console',m=>{if(m.type()==='error'&&diagnostics.console.length<100)diagnostics.console.push(m.text().slice(0,1000));});
 await page.route('**/api/session',r=>r.fulfill({json:{}}));
 await page.route('**/api/v1/**',async route=>{const url=new URL(route.request().url()),p=url.pathname.replace('/api/v1','');if(p.startsWith('/visualization/'))return route.fulfill({response:await route.fetch({url:backend+url.pathname+url.search})});if(p==='/events')return new Promise(()=>{});if(p.endsWith('/messages')&&route.request().method()==='POST'){sent++;return route.fulfill({json:{}});}if(p.endsWith('/history'))return route.fulfill({json:{messages:[]}});if(p==='/state')return route.fulfill({json:{last_seq:0,agents:[{path:'/root/alpha',agent_id:'a1',status:'Idle'}],contexts:{'/root/alpha':{id:'s1',title:'Candidate focus',created_at:1,last_active:1}},display_settings:{collapse_thinking:false,collapse_tool_calls:false,collapse_tool_results:false}}});if(p==='/profiles')return route.fulfill({json:['researcher']});if(p==='/model-config')return route.fulfill({json:{models:[],providers:[]}});if(p==='/settings')return route.fulfill({json:{}});if(p==='/storage/agents')return route.fulfill({json:[]});if(p==='/gateway/status')return route.fulfill({json:{running:true}});return route.fulfill({status:404,json:{error:`unrelated fixture ${p}`}});});
 await page.goto(origin,{waitUntil:'domcontentloaded',timeout:60000});await page.getByRole('navigation',{name:'工作台活动栏'}).getByRole('button',{name:'画布',exact:true}).click();await page.getByRole('textbox',{name:'新项目名称'}).fill('Illustration fit');await page.getByRole('button',{name:'创建项目',exact:true}).click();await page.getByRole('button',{name:'新建画布',exact:true}).click();await page.locator('[data-canvas-surface] .excalidraw:visible').waitFor();
 const project=await page.getByRole('combobox',{name:'可视化项目'}).inputValue(),figure=(await call('GET',`/api/v1/visualization/projects/${project}/figures`))[0].id,path=`/api/v1/visualization/projects/${project}/figures/${figure}`;
 const source=process.env.VISUALIZATION_TEXT_FIT_SCENE,targetId=process.env.VISUALIZATION_TEXT_FIT_ID;
 assert.ok(source&&targetId,'set VISUALIZATION_TEXT_FIT_SCENE and VISUALIZATION_TEXT_FIT_ID for this explicit saved-scene replay');
 const input=JSON.parse(await readFile(source,'utf8')),document=input.scene?.document??input.document??input;
 const initial=await call('GET',path);await call('PUT',path,{expected_revision:initial.metadata.revision,title:'Fit existing text',document});
 page.once('dialog',d=>d.accept());await page.getByRole('button',{name:'重新加载服务端版本',exact:true}).click();
 const formal=await call('GET',path),revision=formal.metadata.revision,find=(doc,id)=>doc.elements.find(e=>e.id===id),prior=find(document,targetId);assert.ok(prior);
 const propose=async(summary,operations)=>{const brief=await call('POST',`${path}/proposals`,{expected_revision:revision,summary,operations});return call('GET',`${path}/proposals/${brief.id}`);};
 const before=await propose('Before fitting text',[]),after=await propose('Fit saved text width',[{op:'fit_text_width',id:targetId}]),fitted=find(after.document,targetId);
 assert.ok(fitted.width<prior.width);
 const restored=structuredClone(after.document),old=structuredClone(prior);
 for(const key of ['x','width','version','versionNonce','updated']){if(Object.hasOwn(old,key))find(restored,targetId)[key]=old[key];else delete find(restored,targetId)[key];}
 assert.deepEqual(restored,document,'only box width, alignment position and revision stamps may change');
 const render=async(candidate,name)=>{
  await call('POST',`${path}/proposals/${candidate.id}/preview-request`,{expected_revision:revision});let result;
  await until(async()=>{result=await call('GET',`${path}/proposals/${candidate.id}/preview?expected_revision=${revision}`);assert.notEqual(result.render_status,'failed');return result.render_status==='ready';},'candidate PNG');
  assert.equal(result.document_sha256,candidate.document_sha256);const bytes=Buffer.from(result.dataURL.split(',')[1],'base64');await writeFile(join(directory,name),bytes);return {width:bytes.readUInt32BE(16),height:bytes.readUInt32BE(20)};
 };
 const beforePng=await render(before,'before.png'),afterPng=await render(after,'after.png');
 assert.deepEqual(await call('GET',path),formal,'preview does not adopt');
 if(process.env.VISUALIZATION_EXPECT_TEXT_CANVAS_SHRINK==='1'){assert.ok(afterPng.width<beforePng.width);assert.equal(afterPng.height,beforePng.height);}
 await page.getByRole('button',{name:'查看 Agent 提案',exact:true}).click();const dialog=page.getByRole('dialog',{name:'Agent 修改预览',exact:true});await dialog.getByRole('button',{name:`${after.summary} · v${revision}`,exact:true}).click();
 await dialog.getByRole('region',{name:'候选画布'}).locator('.excalidraw').waitFor();await dialog.getByRole('button',{name:'采用提案',exact:true}).click();await dialog.waitFor({state:'hidden'});
 const adopted=await call('GET',path);assert.deepEqual(adopted.scene.document,after.document);
 const event=page.waitForEvent('download');await page.getByRole('button',{name:'导出 源文件',exact:true}).click();const exported=JSON.parse(await readFile(await(await event).path(),'utf8'));
 for(const e of after.document.elements){const actual=find(exported,e.id);for(const key of ['x','y','width','height','points','text','originalText','fontSize','fontFamily','lineHeight','customData','frameId','groupIds','startBinding','endBinding'])if(Object.hasOwn(e,key))assert.deepEqual(actual[key],JSON.parse(JSON.stringify(e[key])),`${e.id}/${key}`);}
 assert.deepEqual(exported.files,after.document.files);assert.deepEqual(errors,[]);
 // A separate one-text fixture exercises the actual human editing entry.
 const uiNote={...structuredClone(prior),id:'human-fit-note',x:0,y:0,frameId:null,containerId:null,groupIds:[],boundElements:[],locked:false};
 const created=await call('POST','/test/visualization',{action:'create',project_id:project,title:'Human text width',document:{type:'excalidraw',version:2,elements:[uiNote],files:{}}});
 const uiPath=`/api/v1/visualization/projects/${project}/figures/${created.content.id}`;
 await page.getByRole('button',{name:'刷新画布列表',exact:true}).click();await page.getByRole('button',{name:'Human text width',exact:true}).click();
 const surface=page.locator('[data-canvas-surface] .excalidraw:visible');await surface.waitFor();const box=await surface.boundingBox();
 await page.mouse.click(box.x+box.width*.8,box.y+box.height*.8);await page.keyboard.press('v');await page.keyboard.press('Control+a');
 await page.getByRole('button',{name:'编辑文字与换行',exact:true}).click();const edit=page.getByRole('dialog',{name:'文字与换行',exact:true});
 const inputText=edit.getByRole('textbox',{name:'文字原文',exact:true}),fitButton=edit.getByRole('button',{name:'预览收紧文字框',exact:true});await inputText.waitFor();
 assert.equal(await inputText.inputValue(),prior.originalText??prior.text);assert.equal(await fitButton.isEnabled(),true);
 await inputText.fill('Unsaved wording');assert.equal(await fitButton.isEnabled(),false);await inputText.fill(prior.originalText??prior.text);
 await edit.getByRole('spinbutton',{name:'换行宽度',exact:true}).fill('100');assert.equal(await fitButton.isEnabled(),false);await edit.getByRole('spinbutton',{name:'换行宽度',exact:true}).fill(String(prior.width));
 await edit.getByRole('spinbutton',{name:'换行字号',exact:true}).fill('20');assert.equal(await fitButton.isEnabled(),false);await edit.getByRole('spinbutton',{name:'换行字号',exact:true}).fill(String(prior.fontSize));
 assert.equal(await fitButton.isEnabled(),true);const uiHead=await call('GET',uiPath);await fitButton.click();await dialog.getByRole('region',{name:'候选画布'}).locator('.excalidraw').waitFor();
 assert.deepEqual(await call('GET',uiPath),uiHead,'human entry creates a pending proposal only');
 const uiCandidates=await call('GET',`${uiPath}/proposals`);assert.equal(uiCandidates.length,1);
 const uiProposal=await call('GET',`${uiPath}/proposals/${uiCandidates[0].id}`);assert.ok(uiProposal.document.elements[0].width<prior.width);
 assert.equal(uiProposal.document.elements[0].text,uiHead.scene.document.elements[0].text);assert.equal(uiProposal.document.elements[0].originalText,prior.originalText);
 await page.screenshot({path:join(directory,'human-fit-preview.png'),animations:'disabled'});
 await dialog.getByRole('button',{name:'采用提案',exact:true}).click();await dialog.waitFor({state:'hidden'});assert.deepEqual((await call('GET',uiPath)).scene.document,uiProposal.document);
 await persist('human-fit-proposal.json',uiProposal);
 // The original editing path remains available after fitting.
 const reselect=async()=>{const surface=page.locator('[data-canvas-surface] .excalidraw:visible');const box=await surface.boundingBox();await page.mouse.click(box.x+box.width*.8,box.y+box.height*.8);await page.keyboard.press('v');await page.keyboard.press('Control+a');await page.getByRole('button',{name:'编辑文字与换行',exact:true}).click();};
 await reselect();await inputText.waitFor();const edited='Human edited scope with Greek α and 细胞';await inputText.fill(edited);await edit.getByRole('spinbutton',{name:'换行宽度',exact:true}).fill('180');assert.equal(await fitButton.isEnabled(),false);
 const beforeEdit=await call('GET',uiPath);await edit.getByRole('button',{name:'预览文字修改提案',exact:true}).click();await dialog.getByRole('region',{name:'候选画布'}).locator('.excalidraw').waitFor();assert.deepEqual(await call('GET',uiPath),beforeEdit);
 const editCandidates=await call('GET',`${uiPath}/proposals`),editBrief=editCandidates.find(c=>c.base_revision===beforeEdit.metadata.revision&&c.summary==='修改选中文字并按可用宽度换行');assert.ok(editBrief);
 const editedProposal=await call('GET',`${uiPath}/proposals/${editBrief.id}`);assert.equal(editedProposal.document.elements[0].originalText,edited);assert.ok(editedProposal.document.elements[0].text.includes('\n'));
 await dialog.getByRole('button',{name:'采用提案',exact:true}).click();await dialog.waitFor({state:'hidden'});assert.deepEqual((await call('GET',uiPath)).scene.document,editedProposal.document);
 const template=document.elements.find(e=>e.type==='rectangle');assert.ok(template);
 const shape={...structuredClone(template),id:'human-fit-box',x:0,y:0,width:220,height:100,frameId:null,groupIds:[],boundElements:[{id:'bound-caption',type:'text'}],customData:{},angle:0,locked:false,isDeleted:false};
 const boundText={...uiNote,id:'bound-caption',x:10,y:40,width:200,height:20,text:'Bound caption',originalText:'Bound caption',containerId:shape.id};
 await call('POST','/test/visualization',{action:'create',project_id:project,title:'Bound text width',document:{type:'excalidraw',version:2,elements:[shape,boundText],files:{}}});
 await page.getByRole('button',{name:'刷新画布列表',exact:true}).click();await page.getByRole('button',{name:'Bound text width',exact:true}).click();await page.locator('[data-canvas-surface] .excalidraw:visible').waitFor();await reselect();await inputText.waitFor();
 assert.equal(await fitButton.count(),0,'bound text retains container reflow instead of independent fitting');assert.equal(await edit.getByRole('button',{name:'预览文字修改提案',exact:true}).count(),1);await page.keyboard.press('Escape');
 await checkHumanBatchReflow({page,call,project,uiNote,shape,edit,dialog,reselect,persist,directory});
 assert.equal(sent,0,'human text editing never sends Agent messages');

 await persist('before.json',document);await persist('proposal.json',after);await persist('source.json',exported);
 await persist('result.json',{passed:true,model_used:false,target_id:targetId,before_width:prior.width,after_width:fitted.width,before_png:beforePng,after_png:afterPng,human_entry_verified:true,batch_reflow_verified:true,batch_invalid_member_rejected:true,batch_limit_rejected:true,batch_native_failure_atomic:true,batch_stale_revision_rejected:true,unsaved_form_edits_disable_fit:true,original_reflow_verified:true,bound_text_fit_hidden:true,scope:'one explicit text-width edit in the supplied saved scene; other elements/files/root metadata exact before adoption; native PNGs, GUI adoption and editable export verified; original input file untouched'});
 console.log(`fit-existing-text: passed; artifacts=${directory}`);
} catch(error){await persist('failure.json',{error:String(error),stack:error.stack,diagnostics,native_output:output,vite_output:viteOutput});if(page)await page.screenshot({path:join(directory,'failure.png'),animations:'disabled'}).catch(()=>{});console.error(`fit-existing-text: failed; artifacts=${directory}`);throw error;}finally{if(browser)await browser.close();for(const p of [vite,native]){if(p.exitCode===null&&p.signalCode===null){p.kill();await new Promise(resolve=>p.once('exit',resolve));}}}
