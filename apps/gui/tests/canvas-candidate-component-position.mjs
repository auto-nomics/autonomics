import {checkPortableSceneExport} from './helpers/canvas-live-source-review.mjs';
// Adopt/export an explicitly reviewed real-model checkpoint; no new model invocation.
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
 const parent=await call('GET',`${path}/proposals/${result.id}`),node=parent.document.elements.find(e=>e.id==='node-1');assert.ok(node.customData.visualAssetElementId);
 await page.getByRole('button',{name:'查看 Agent 提案',exact:true}).click();const dialog=page.getByRole('dialog',{name:'Agent 修改预览',exact:true});await dialog.locator(`button[data-proposal-id="${parent.id}"]`).click();await dialog.getByRole('region',{name:'候选画布',exact:true}).locator('.excalidraw').waitFor();
 await dialog.getByRole('textbox',{name:'搜索候选对象',exact:true}).fill(node.id);await dialog.getByRole('combobox',{name:'候选对象',exact:true}).selectOption(node.id);
 const editor=dialog.locator('details[aria-label="候选素材组件位置"]');await editor.locator('summary').click();
 const y=editor.getByRole('spinbutton',{name:'候选素材组件Y',exact:true}),button=editor.getByRole('button',{name:'预览素材组件移动',exact:true});await y.fill('');assert.equal(await button.isEnabled(),false);await y.fill(String(node.y-30));
 const request=page.waitForRequest(r=>r.method()==='POST'&&new URL(r.url()).pathname.endsWith('/revisions'));await button.click();assert.deepEqual((await request).postDataJSON().operations,[{op:'translate',element_ids:[node.id],dx:0,dy:-30}]);
 let child;await until(async()=>{child=(await call('GET',`${path}/proposals`)).find(p=>p.parent_proposal_id===parent.id);return Boolean(child);},'component child');child=await call('GET',`${path}/proposals/${child.id}`);
 assert.equal(child.parent_document_sha256,parent.document_sha256);assert.deepEqual(child.document.files,parent.document.files);
 const moving=new Set([node.id,node.customData.visualAssetElementId,...node.customData.visualLabelIds]);
 const endpoint=(e,start)=>{const p=start?e.points[0]:e.points.at(-1);return[e.x+p[0],e.y+p[1]];};
 for(const old of parent.document.elements){const next=child.document.elements.find(e=>e.id===old.id);assert.ok(next);
  if(moving.has(old.id)){assert.equal(next.x,old.x);assert.equal(next.y,old.y-30);assert.equal(next.width,old.width);assert.equal(next.height,old.height);}
  else if(old.type!=='arrow'&&!(old.type==='text'&&old.containerId?.startsWith('edge-')))assert.deepEqual(next,old);
  for(const key of ['text','originalText','fontSize','fontFamily','frameId','startBinding','endBinding','startArrowhead','endArrowhead'])assert.deepEqual(next[key],old[key]);
  if(old.type==='arrow')for(const [field,start]of[['startBinding',true],['endBinding',false]]){const a=endpoint(old,start),b=endpoint(next,start);assert.ok(Math.abs(b[0]-a[0])<1e-6);assert.ok(Math.abs(b[1]-a[1]-(old[field]?.elementId===node.id?-30:0))<1e-6);}
 }
 assert.deepEqual(await call('GET',path),formal);assert.deepEqual(await call('GET',`${path}/proposals/${parent.id}`),parent);assert.equal(sent,0);
 await until(async()=>await dialog.locator('details[aria-label="候选素材组件位置"]').getAttribute('data-candidate-id')===child.id,'child selected');
 await call('POST',`${path}/proposals/${child.id}/preview-request`,{expected_revision:revision});let preview;await until(async()=>{preview=await call('GET',`${path}/proposals/${child.id}/preview?expected_revision=${revision}`);assert.notEqual(preview.render_status,'failed');return preview.render_status==='ready';},'child PNG');assert.equal(preview.document_sha256,child.document_sha256);await writeFile(join(directory,'component-position.png'),Buffer.from(preview.dataURL.split(',')[1],'base64'));await page.screenshot({path:join(directory,'component-position-gui.png'),animations:'disabled'});
 await dialog.getByRole('button',{name:'采用提案',exact:true}).click();await dialog.waitFor({state:'hidden'});assert.deepEqual((await call('GET',path)).scene.document,child.document);
 const download=page.waitForEvent('download');await page.getByRole('button',{name:'导出 源文件',exact:true}).click();const exported=JSON.parse(await readFile(await(await download).path(),'utf8'));checkPortableSceneExport(exported,child.document);await persist('source.json',exported);await persist('child.json',child);assert.deepEqual(errors,[]);await persist('result.json',{passed:true,model_used:false,scope:'saved real candidate; GUI component translation, immutable parent, far endpoints, text/fonts/files, PNG/manual adoption/source export; no claim of whole figure completeness'});console.log(`component-position-ui: passed; artifacts=${directory}`);
} catch(error){await persist('failure.json',{error:String(error),stack:error.stack,diagnostics,native_output:output,vite_output:viteOutput});if(page)await page.screenshot({path:join(directory,'failure.png'),animations:'disabled'}).catch(()=>{});console.error(`candidate-replay: failed; artifacts=${directory}`);throw error;}finally{if(page)await page.unrouteAll({behavior:"ignoreErrors"});if(browser)await browser.close();for(const p of [vite,native]){if(p.exitCode===null&&p.signalCode===null){p.kill();await new Promise(resolve=>p.once('exit',resolve));}}}
