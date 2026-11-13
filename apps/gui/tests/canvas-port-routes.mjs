// Explicit reroute replay of saved native arrows; no model invocation.
import assert from 'node:assert/strict';
import {chromium} from 'playwright';
import {spawn} from 'node:child_process';
import {mkdtemp,readFile,writeFile} from 'node:fs/promises';
import {existsSync} from 'node:fs';
import {tmpdir} from 'node:os';
import {join} from 'node:path';
import {fileURLToPath} from 'node:url';
const binary=process.env.VISUALIZATION_NATIVE_BIN||fileURLToPath(new URL('../../../target/debug/examples/visualization_native',import.meta.url));
const directory=await mkdtemp(join(tmpdir(),'autonomics-port-routes-')),origin='http://127.0.0.1:1462';
const native=spawn(binary,[join(directory,'figures.sqlite')],{stdio:['ignore','pipe','pipe']}),vite=spawn(process.execPath,['node_modules/vite/bin/vite.js','--host','127.0.0.1','--port','1462','--strictPort'],{stdio:['ignore','pipe','pipe']});
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
 const source=process.env.VISUALIZATION_ROUTE_SCENE;assert.ok(source,'set VISUALIZATION_ROUTE_SCENE for this saved-scene replay');
 const ids=(process.env.VISUALIZATION_ROUTE_IDS??'edge-1,edge-3').split(',');assert.ok(ids.length&&ids.length<=10&&new Set(ids).size===ids.length);
 const input=JSON.parse(await readFile(source,'utf8')),document=input.scene?.document??input.document??input;
 const find=(doc,id)=>doc.elements.find(e=>e.id===id);for(const id of ids)assert.equal(find(document,id)?.type,'arrow');
 const initial=await call('GET',path);await call('PUT',path,{expected_revision:initial.metadata.revision,title:'Port route replay',document});
 page.once('dialog',d=>d.accept());await page.getByRole('button',{name:'重新加载服务端版本',exact:true}).click();
 const formal=await call('GET',path),revision=formal.metadata.revision;
 const propose=async(summary,operations)=>{const brief=await call('POST',`${path}/proposals`,{expected_revision:revision,summary,operations});return call('GET',`${path}/proposals/${brief.id}`);};
 const before=await propose('Before local port routes',[]);
 const revised=await call('POST',`${path}/proposals/${before.id}/revisions`,{expected_revision:revision,parent_document_sha256:before.document_sha256,summary:'Local port routes',markup_ids:[],operations:ids.map(id=>({op:'reroute',id}))});
 const after=await call('GET',`${path}/proposals/${revised.id}`);
 const allowed=new Set(ids),metrics=[];
 for(const id of ids){const old=find(document,id),arrow=find(after.document,id),points=arrow.points;
  const length=p=>p.slice(1).reduce((sum,b,i)=>sum+Math.hypot(b[0]-p[i][0],b[1]-p[i][1]),0);
  const direct=Math.abs(points[0][0]-points.at(-1)[0])+Math.abs(points[0][1]-points.at(-1)[1]);
  assert.ok(length(points)<=direct+150,`${id}: local route instead of a return loop`);
  assert.ok(Math.hypot(points.at(-1)[0]-points.at(-2)[0],points.at(-1)[1]-points.at(-2)[1])>=24,`${id}: visible terminal span`);
  for(const key of ['startArrowhead','endArrowhead','customData'])assert.deepEqual(arrow[key],old[key]);for(const key of ['startBinding','endBinding'])assert.equal(arrow[key].elementId,old[key].elementId);
  for(const label of document.elements.filter(e=>e.type==='text'&&e.containerId===id)){allowed.add(label.id);const changed=find(after.document,label.id);for(const key of ['text','originalText','width','height','fontFamily','fontSize','lineHeight'])assert.deepEqual(changed[key],label[key]);}
  metrics.push({id,before_length:length(old.points),after_length:length(points),manhattan:direct});
 }
 for(const old of document.elements)if(!allowed.has(old.id))assert.deepEqual(find(after.document,old.id),old);
 assert.deepEqual(after.document.files,document.files);
 assert.deepEqual(Object.fromEntries(Object.entries(after.document).filter(([k])=>k!=='elements')),Object.fromEntries(Object.entries(document).filter(([k])=>k!=='elements')));
 const render=async(candidate,name)=>{await call('POST',`${path}/proposals/${candidate.id}/preview-request`,{expected_revision:revision});let png;await until(async()=>{png=await call('GET',`${path}/proposals/${candidate.id}/preview?expected_revision=${revision}`);assert.notEqual(png.render_status,'failed');return png.render_status==='ready';},'route PNG');assert.equal(png.document_sha256,candidate.document_sha256);await writeFile(join(directory,name),Buffer.from(png.dataURL.split(',')[1],'base64'));};
 await render(before,'before.png');await render(after,'after.png');assert.deepEqual(await call('GET',path),formal);
 await page.getByRole('button',{name:'查看 Agent 提案',exact:true}).click();const dialog=page.getByRole('dialog',{name:'Agent 修改预览',exact:true});await dialog.getByRole('button',{name:`${after.summary} · v${revision} · 修订`,exact:true}).click();await dialog.getByRole('region',{name:'候选画布'}).locator('.excalidraw').waitFor();await dialog.getByRole('button',{name:'采用提案',exact:true}).click();await dialog.waitFor({state:'hidden'});assert.deepEqual((await call('GET',path)).scene.document,after.document);
 const event=page.waitForEvent('download');await page.getByRole('button',{name:'导出 源文件',exact:true}).click();const exported=JSON.parse(await readFile(await(await event).path(),'utf8'));
 for(const e of after.document.elements){const actual=find(exported,e.id);for(const key of ['x','y','width','height','points','text','originalText','customData','startBinding','endBinding'])if(Object.hasOwn(e,key))assert.deepEqual(actual[key],JSON.parse(JSON.stringify(e[key])),`${e.id}/${key}`);}
 assert.deepEqual(exported.files,after.document.files);assert.deepEqual(errors,[]);
 await persist('proposal.json',after);await persist('source.json',exported);await persist('result.json',{passed:true,model_used:false,routes:metrics,scope:'explicit saved native arrow reroute; scientific text/metadata, endpoints, existing images/files and unrelated objects preserved; PNGs, GUI adoption and native export verified; no new scientific or model-workflow validation'});
 console.log(`port-routes: passed; artifacts=${directory}`);
} catch(error){await persist('failure.json',{error:String(error),stack:error.stack,diagnostics,native_output:output,vite_output:viteOutput});if(page)await page.screenshot({path:join(directory,'failure.png'),animations:'disabled'}).catch(()=>{});console.error(`port-routes: failed; artifacts=${directory}`);throw error;}finally{if(browser)await browser.close();for(const p of [vite,native]){if(p.exitCode===null&&p.signalCode===null){p.kill();await new Promise(resolve=>p.once('exit',resolve));}}}
