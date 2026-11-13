// Derive an isolated scientific label fixture from an explicit saved scene; no model.
import assert from 'node:assert/strict';
import {chromium} from 'playwright';
import {spawn} from 'node:child_process';
import {mkdtemp,readFile,writeFile} from 'node:fs/promises';
import {existsSync} from 'node:fs';
import {tmpdir} from 'node:os';
import {join} from 'node:path';
import {fileURLToPath} from 'node:url';
const binary=process.env.VISUALIZATION_NATIVE_BIN||fileURLToPath(new URL('../../../target/debug/examples/visualization_native',import.meta.url));
const directory=await mkdtemp(join(tmpdir(),'autonomics-plan-review-compiled-')),origin='http://127.0.0.1:1459';
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
 const source=process.env.VISUALIZATION_PLAN_REVIEW_COMPILED_SOURCE;
 assert.ok(source,'set VISUALIZATION_PLAN_REVIEW_COMPILED_SOURCE to explicit successful plan-review artifacts');
 const original=JSON.parse(await readFile(join(source,'review-original-plan.json'),'utf8'));
 const updated=JSON.parse(await readFile(join(source,'review-final-plan.json'),'utf8'));
 const batch=process.env.VISUALIZATION_PLAN_REVIEW_COMPILED_BATCH==='1',indices=batch?[0,3]:[0];
 if(batch){const write=JSON.parse(await readFile(join(source,'plan-review-batch-write.json'),'utf8'));assert.equal(write.name,'visualization_update_plan_labels');assert.deepEqual(write.input.changes.map(c=>c.plan_field).sort(),indices.map(i=>`edges[${i}].label`));for(const change of write.input.changes){const index=Number(change.plan_field.match(/^edges\[(\d+)\]\.label$/)[1]);assert.equal(change.expected_text,original.document.edges[index].label);assert.equal(change.text,updated.document.edges[index].label);}}
 const expected=structuredClone(original.document);for(const index of indices)expected.edges[index].label=updated.document.edges[index].label;
 assert.notEqual(original.document.edges[0].label,updated.document.edges[0].label);assert.deepEqual(updated.document,expected);
 await page.getByRole('button',{name:'保存画布',exact:true}).click();
 const formal=await call('GET',path),revision=formal.metadata.revision;
 const compile=async(document,version,summary)=>{
  const plan=await call('PUT',`${path}/planning`,{expected_revision:revision,expected_plan_version:version,document});
  const brief=await call('POST',`${path}/planning/sketch`,{expected_revision:revision,expected_plan_version:plan.version,summary});
  return {plan,candidate:await call('GET',`${path}/proposals/${brief.id}`)};
 };
 const first=await compile(original.document,0,'Original reviewed plan'),second=await compile(updated.document,1,'Selected label revision');
 const before=first.candidate,after=second.candidate;
 const find=(doc,id)=>doc.elements.find(e=>e.id===id);
 assert.deepEqual(before.document.elements.map(e=>e.id),after.document.elements.map(e=>e.id));
 for(const old of before.document.elements){
  const current=find(after.document,old.id);assert.equal(current.type,old.type);
  if(old.type==='text'&&!indices.some(index=>old.id===`edge-label-${index}`))assert.equal(current.originalText,old.originalText,old.id);
  for(const key of ['strokeStyle','startArrowhead','endArrowhead'])assert.deepEqual(current[key],old[key],`${old.id}/${key}`);
  for(const key of ['startBinding','endBinding'])assert.equal(current[key]?.elementId,old[key]?.elementId,`${old.id}/${key}`);
  const prior=structuredClone(old.customData??{}),next=structuredClone(current.customData??{});delete prior.planningSource;delete next.planningSource;if(prior.scientificPlan){const expectedPlan=structuredClone(prior.scientificPlan);for(const index of indices)expectedPlan.edges[index].label=updated.document.edges[index].label;assert.deepEqual(next.scientificPlan,expectedPlan,'embedded scientific plan changes only the selected label');delete prior.scientificPlan;delete next.scientificPlan;}assert.deepEqual(next,prior,`${old.id}/scientific metadata`);
 }
 for(const index of indices)assert.equal(find(after.document,`edge-label-${index}`).originalText,updated.document.edges[index].label);
 const rowFrames=new Map();for(const [index,panel] of updated.document.panels.entries()){const frame=find(after.document,`panel-${index}`);const previous=rowFrames.get(panel.row);if(previous){assert.equal(frame.y,previous.y);assert.equal(frame.height,previous.height,'same-row panel frame bottoms align');}else rowFrames.set(panel.row,frame);}
 const beforeLines=find(before.document,'edge-label-0').text.split('\n').length,afterLines=find(after.document,'edge-label-0').text.split('\n').length;assert.ok(afterLines<beforeLines);
 assert.deepEqual(after.document.elements[0].customData.planningSource.document,updated.document);
 const render=async(candidate,name)=>{
  await call('POST',`${path}/proposals/${candidate.id}/preview-request`,{expected_revision:revision});let result;
  await until(async()=>{result=await call('GET',`${path}/proposals/${candidate.id}/preview?expected_revision=${revision}`);assert.notEqual(result.render_status,'failed');return result.render_status==='ready';},'candidate PNG');
  assert.equal(result.document_sha256,candidate.document_sha256);await writeFile(join(directory,name),Buffer.from(result.dataURL.split(',')[1],'base64'));
 };
 await render(before,'before.png');await render(after,'after.png');assert.deepEqual(await call('GET',path),formal);
 await page.getByRole('button',{name:'绘图计划',exact:true}).click();const planningDialog=page.getByRole('dialog',{name:'科研绘图计划',exact:true});
 const differences=planningDialog.locator('details[aria-label="已保存计划版本差异"]');await differences.locator('summary').click();await differences.getByRole('table').waitFor();assert.equal(await differences.getByRole('row').count(),indices.length+1);
 for(const index of indices){const row=differences.getByRole('row').filter({has:page.getByRole('rowheader',{name:`关系 ${index+1} · 标签`,exact:true})});await row.waitFor();assert.ok((await row.textContent()).includes(original.document.edges[index].label));assert.ok((await row.textContent()).includes(updated.document.edges[index].label));}
 assert.deepEqual((await call('GET',`${path}/planning`)).document,updated.document);assert.deepEqual(await call('GET',path),formal);assert.equal((await call('GET',`${path}/proposals`)).length,2);await page.screenshot({path:join(directory,'plan-label-changes.png'),animations:'disabled'});await planningDialog.getByRole('button',{name:'关闭计划',exact:true}).click();

 await page.getByRole('button',{name:'查看 Agent 提案',exact:true}).click();const dialog=page.getByRole('dialog',{name:'Agent 修改预览',exact:true});await dialog.getByRole('button',{name:`${after.summary} · v${revision}`,exact:true}).click();
 await dialog.getByRole('region',{name:'候选画布'}).locator('.excalidraw').waitFor();await dialog.getByRole('button',{name:'采用提案',exact:true}).click();await dialog.waitFor({state:'hidden'});
 const adopted=await call('GET',path);assert.deepEqual(adopted.scene.document,after.document);
 const event=page.waitForEvent('download');await page.getByRole('button',{name:'导出 源文件',exact:true}).click();const exported=JSON.parse(await readFile(await(await event).path(),'utf8'));
 for(const e of after.document.elements){const actual=find(exported,e.id);for(const key of ['x','y','width','height','points','text','originalText','fontSize','fontFamily','lineHeight','customData','frameId','groupIds','startBinding','endBinding'])if(Object.hasOwn(e,key))assert.deepEqual(actual[key],JSON.parse(JSON.stringify(e[key])),`${e.id}/${key}`);}
 assert.deepEqual(errors,[]);assert.equal(sent,0);
 await persist('before-plan.json',first.plan);await persist('after-plan.json',second.plan);await persist('source.json',exported);
 await persist('result.json',{passed:true,model_used:false,scope:'compile explicit original and actual model-revised saved plans, preview both, GUI adopt and native source export; no fresh scientific generation',before_lines:beforeLines,after_lines:afterLines,saved_label_comparison_verified:true,batch_revised_labels_verified:batch,same_row_frame_alignment_verified:true,all_other_labels_and_scientific_metadata_preserved:true});console.log(`plan-review-compiled: passed; artifacts=${directory}`);
} catch(error){await persist('failure.json',{error:String(error),stack:error.stack,diagnostics,native_output:output,vite_output:viteOutput});if(page)await page.screenshot({path:join(directory,'failure.png'),animations:'disabled'}).catch(()=>{});console.error(`plan-review-compiled: failed; artifacts=${directory}`);throw error;}finally{if(browser)await browser.close();for(const p of [vite,native]){if(p.exitCode===null&&p.signalCode===null){p.kill();await new Promise(resolve=>p.once('exit',resolve));}}}
