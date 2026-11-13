// Batch frame-boundary fitting against an explicit real-model scene; no adoption or model call.
import assert from 'node:assert/strict';
import {chromium} from 'playwright';
import {spawn} from 'node:child_process';
import {mkdtemp,readFile,writeFile} from 'node:fs/promises';
import {existsSync} from 'node:fs';
import {tmpdir} from 'node:os';
import {join} from 'node:path';
import {fileURLToPath} from 'node:url';
const binary=process.env.VISUALIZATION_NATIVE_BIN||fileURLToPath(new URL('../../../target/debug/examples/visualization_native',import.meta.url));
const directory=await mkdtemp(join(tmpdir(),'autonomics-candidate-replay-')),origin='http://127.0.0.1:1465';
const native=spawn(binary,[join(directory,'figures.sqlite')],{stdio:['ignore','pipe','pipe']}),vite=spawn(process.execPath,['node_modules/vite/bin/vite.js','--host','127.0.0.1','--port','1465','--strictPort'],{stdio:['ignore','pipe','pipe']});
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

 const sourcePath=process.env.VISUALIZATION_COMPONENT_SOURCE;assert.ok(sourcePath);
 const input=JSON.parse(await readFile(sourcePath,'utf8')),document=input.document??input.scene?.document??input;
 const initial=await call('GET',path);await call('PUT',path,{expected_revision:initial.metadata.revision,title:'Component position',document});page.once('dialog',d=>d.accept());await page.getByRole('button',{name:'重新加载服务端版本',exact:true}).click();await page.locator('[data-canvas-surface] .excalidraw:visible').waitFor();
 const formal=await call('GET',path),revision=formal.metadata.revision;
 const result=await call('POST','/test/saved-candidate',{project_id:project,figure_id:figure,expected_revision:revision,title:'Component position review',document});
 const parent=await call('GET',`${path}/proposals/${result.id}`),frames=parent.document.elements.filter(e=>e.type==='frame'&&!e.isDeleted);
 assert.ok(frames.length>=3);
 await page.getByRole('button',{name:'查看 Agent 提案',exact:true}).click();const dialog=page.getByRole('dialog',{name:'Agent 修改预览',exact:true});await dialog.locator(`button[data-proposal-id="${parent.id}"]`).click();
 const card=dialog.locator('details[aria-label="候选分面对齐分布"]');await card.locator('summary').click();await card.getByRole('combobox',{name:'候选分面布局方式',exact:true}).selectOption('fit_contents');
 const submit=card.getByRole('button',{name:'预览分面布局',exact:true});assert.equal(await submit.isEnabled(),false);
 const checkboxes=await card.locator('fieldset').first().getByRole('checkbox').all();
 await checkboxes[0].check();await checkboxes[1].check();
 await card.getByRole('combobox',{name:'候选分面布局方式',exact:true}).selectOption('distribute_horizontal');
 assert.equal(await submit.isEnabled(),false);await card.getByRole('status').filter({hasText:'至少三个分面'}).waitFor();
 for(const checkbox of checkboxes.slice(2))await checkbox.check();assert.equal(await submit.isEnabled(),true);
 await card.getByRole('combobox',{name:'候选分面布局方式',exact:true}).selectOption('fit_contents');
 const padding=card.getByRole('spinbutton',{name:'所选分面统一内容边距',exact:true});await padding.fill('');assert.equal(await submit.isEnabled(),false);await padding.fill('24');
 const request=page.waitForRequest(r=>r.method()==='POST'&&new URL(r.url()).pathname.endsWith('/revisions'));await submit.click();assert.deepEqual((await request).postDataJSON().operations,frames.map(e=>({op:'fit_frame',id:e.id,padding:24})));
 let child;await until(async()=>{child=(await call('GET',`${path}/proposals`)).find(p=>p.parent_proposal_id===parent.id);return Boolean(child);},'batch fit child');child=await call('GET',`${path}/proposals/${child.id}`);
 const changed=[];for(const before of parent.document.elements){const after=child.document.elements.find(e=>e.id===before.id);if(before.type!=='frame')assert.deepEqual(after,before);else{assert.equal(after.x,before.x);assert.equal(after.y,before.y);assert.ok(after.width<=before.width&&after.height<=before.height);if(after.width<before.width||after.height<before.height)changed.push({id:before.id,before:[before.width,before.height],after:[after.width,after.height]});}}
 assert.ok(changed.length>0);assert.deepEqual(child.document.files,parent.document.files);assert.deepEqual(await call('GET',`${path}/proposals/${parent.id}`),parent);assert.deepEqual(await call('GET',path),formal);
 await until(async()=>await dialog.locator(`button[data-proposal-id="${child.id}"]`).count()>0,'child selected');
 await call('POST',`${path}/proposals/${child.id}/preview-request`,{expected_revision:revision});let preview;await until(async()=>{preview=await call('GET',`${path}/proposals/${child.id}/preview?expected_revision=${revision}`);assert.notEqual(preview.render_status,'failed');return preview.render_status==='ready';},'batch fit PNG');await writeFile(join(directory,'batch-fit.png'),Buffer.from(preview.dataURL.split(',')[1],'base64'));
 assert.deepEqual(errors,[]);assert.equal(sent,0);await persist('child.json',child);await persist('result.json',{passed:true,model_used:false,changed,distribution_requires_three_frames:true,all_nonframe_objects_exactly_preserved:true,parent_and_formal_unchanged:true});console.log(`batch-frame-fit: passed; artifacts=${directory}`);
} catch(error){await persist('failure.json',{error:String(error),stack:error.stack,diagnostics,native_output:output,vite_output:viteOutput});if(page)await page.screenshot({path:join(directory,'failure.png'),animations:'disabled'}).catch(()=>{});console.error(`candidate-replay: failed; artifacts=${directory}`);throw error;}finally{if(page)await page.unrouteAll({behavior:"ignoreErrors"});if(browser)await browser.close();for(const p of [vite,native]){if(p.exitCode===null&&p.signalCode===null){p.kill();await new Promise(resolve=>p.once('exit',resolve));}}}
