import {readCanvasPreparedPrompt,readCanvasPreparedContext} from './helpers/canvas-prepared-context.mjs';
// Native candidate object focus and exact scoped feedback; no model invocation.
import assert from 'node:assert/strict';
import {chromium} from 'playwright';
import {spawn} from 'node:child_process';
import {mkdtemp,readFile,writeFile} from 'node:fs/promises';
import {existsSync} from 'node:fs';
import {tmpdir} from 'node:os';
import {join} from 'node:path';
import {fileURLToPath} from 'node:url';
const source=process.env.VISUALIZATION_FIT_SCENE||fileURLToPath(new URL('./fixtures/scientific-egfr-mapk-illustrated.json',import.meta.url));
const binary=process.env.VISUALIZATION_NATIVE_BIN||fileURLToPath(new URL('../../../target/debug/examples/visualization_native',import.meta.url));
const directory=await mkdtemp(join(tmpdir(),'autonomics-caption-preferences-')),origin='http://127.0.0.1:1456';
const native=spawn(binary,[join(directory,'figures.sqlite')],{stdio:['ignore','pipe','pipe']}),vite=spawn(process.execPath,['node_modules/vite/bin/vite.js','--host','127.0.0.1','--port','1456','--strictPort'],{stdio:['ignore','pipe','pipe']});
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
 const input=JSON.parse(await readFile(source,'utf8')),document=input.scene?.document??input.document??input;
 const anchor=document.elements.find(e=>e.customData?.scientificId==='egfr');assert.ok(anchor);
 if(process.env.VISUALIZATION_PLAN_REVIEW==='1') {
  for(const element of document.elements) if(element.customData) delete element.customData.planningSource;
  const label=document.elements.find(e=>e.type==='text'&&(e.containerId===anchor.id||anchor.customData.visualLabelIds?.includes(e.id)));
  assert.ok(label,'fixture must retain a scientific label');
  anchor.customData.planningSource={version:7,based_on_revision:0,document:{nodes:[{id:'egfr',label:`${label.originalText??label.text} (confirmed)`}],edges:[]}};
 }
 const initial=await call('GET',path);await call('PUT',path,{expected_revision:initial.metadata.revision,title:'Focus figure',document});page.once('dialog',d=>d.accept());await page.getByRole('button',{name:'重新加载服务端版本',exact:true}).click();
 const formal=await call('GET',path),revision=formal.metadata.revision;
 const contexts=[];
 for(const [style,layout] of [['concise','auto'],['concise','compact'],['detailed','horizontal'],['detailed','vertical']]){
  await page.getByRole('tab',{name:'Focus figure',exact:true}).click();
  const toggle=page.getByRole('button',{name:'Agent 绘图',exact:true});if(await toggle.getAttribute('aria-expanded')!=='true')await toggle.click();
  const panel=page.getByRole('complementary',{name:'画布 Agent',exact:true});
  const preference=panel.getByRole('combobox',{name:'图内说明风格',exact:true});if(layout==='auto')assert.equal(await preference.inputValue(),'concise');else await preference.selectOption(style);
  const composition=panel.getByRole('combobox',{name:'构图偏好',exact:true});if(layout==='auto')assert.equal(await composition.inputValue(),'auto');else await composition.selectOption(layout);
  await panel.getByRole('textbox',{name:'绘图需求',exact:true}).fill('请整理图注，保留全部科学含义和来源。');
  await panel.getByRole('button',{name:'交给 Agent',exact:true}).click();
  const composer=page.getByPlaceholder('输入消息…');await composer.waitFor();const draft=await readCanvasPreparedPrompt(page);
  const context=JSON.parse(draft.slice(draft.lastIndexOf('\n\n{')+2));assert.equal(context.composition_preferences.caption_style,style);assert.equal(context.composition_preferences.layout_preference,layout);assert.ok(draft.includes({auto:'用户选择自动构图',compact:'用户选择紧凑分面',horizontal:'用户选择横向流程',vertical:'用户选择纵向流程'}[layout]));assert.equal(context.composition_preferences.preserve_complete_sources,true);assert.equal(context.composition_preferences.preserve_confirmed_scientific_labels,true);
  assert.ok(draft.includes('本次已读来源未见')&&draft.includes('可能'));
  assert.deepEqual(context.follow_up_inputs.read,{action:'read',project_id:project,figure_id:figure,expected_revision:revision});
  assert.equal(context.project_id,project);assert.equal(context.figure_id,figure);assert.equal(context.observed_revision,revision);
  assert.ok(draft.includes(style==='concise'?'用户选择简洁说明':'用户选择详细说明'));assert.ok(draft.includes('物种/刺激范围、终点和关键不确定性必须在画面中可见'));
  const after=await call('GET',path);assert.equal(after.metadata.revision,revision);assert.deepEqual(after.scene.document,formal.scene.document);assert.equal(sent,0);
  contexts.push(context);await composer.fill('');
 }
 // A separate saved one-object figure verifies the actual selection handoff.
 const selectedNote={...structuredClone(document.elements.find(e=>e.type==='text')),id:'selected-handoff-note',x:0,y:0,containerId:null,frameId:null,groupIds:[],boundElements:[],locked:false};
 const created=await call('POST','/test/visualization',{action:'create',project_id:project,title:'Selected read handoff',document:{type:'excalidraw',version:2,elements:[selectedNote],files:{}}});
 const selectedId=created.content.id,selectedPath=`/api/v1/visualization/projects/${project}/figures/${selectedId}`;
 await page.getByRole('button',{name:'刷新画布列表',exact:true}).click();await page.getByRole('button',{name:'Selected read handoff',exact:true}).click();
 const surface=page.locator('[data-canvas-surface] .excalidraw:visible');await surface.waitFor();const box=await surface.boundingBox();await page.mouse.click(box.x+box.width*.8,box.y+box.height*.8);await page.keyboard.press('v');await page.keyboard.press('Control+a');
 const selectedBefore=await call('GET',selectedPath);
 await page.getByRole('button',{name:'Agent 绘图',exact:true}).click();const selectedPanel=page.getByRole('complementary',{name:'画布 Agent',exact:true});
 await selectedPanel.getByRole('textbox',{name:'绘图需求',exact:true}).fill('仅核对选中文字，保留其他对象。');await selectedPanel.getByRole('button',{name:'交给 Agent',exact:true}).click();
 const selectedComposer=page.getByPlaceholder('输入消息…');await selectedComposer.waitFor();const selectedDraft=await selectedComposer.inputValue(),selectedContext=await readCanvasPreparedContext(page);
 assert.deepEqual(selectedContext.follow_up_inputs.read,{action:'read',project_id:project,figure_id:selectedId,expected_revision:selectedBefore.metadata.revision,element_ids:[selectedNote.id]});
 assert.deepEqual(await call('GET',selectedPath),selectedBefore);assert.equal(sent,0);await selectedComposer.fill('');await persist('selected-context.json',selectedContext);
 await persist('contexts.json',contexts);await persist('result.json',{passed:true,model_used:false,sent_messages:sent,scope:'real native figure/version/preview handoff, both caption preferences, all four layout preferences and no implicit scene/content mutation'});console.log(`caption-preferences: passed; artifacts=${directory}`);
} catch(error){await persist('failure.json',{error:String(error),stack:error.stack,diagnostics,native_output:output,vite_output:viteOutput});if(page)await page.screenshot({path:join(directory,'failure.png'),animations:'disabled'}).catch(()=>{});console.error(`caption-preferences: failed; artifacts=${directory}`);throw error;}finally{if(browser)await browser.close();for(const p of [vite,native]){if(p.exitCode===null&&p.signalCode===null){p.kill();await new Promise(resolve=>p.once('exit',resolve));}}}
