import {rasterizeFixtureImage} from './helpers/canvas-raster-fixture.mjs';
// Native rendering/round-trip of explicit artwork bounds; no model decisions are simulated.
import assert from 'node:assert/strict';
import {chromium} from 'playwright';
import {spawn} from 'node:child_process';
import {mkdtemp,readFile,writeFile} from 'node:fs/promises';
import {existsSync} from 'node:fs';
import {tmpdir} from 'node:os';
import {join} from 'node:path';
import {fileURLToPath} from 'node:url';
const bioartMode=process.env.VISUALIZATION_LAYOUT_BIOART==='1';
const rasterMode=process.env.VISUALIZATION_LAYOUT_RASTER==='1';
const editGeometry=process.env.VISUALIZATION_FIT_GEOMETRY==='1';
const manualFit=process.env.VISUALIZATION_FIT_MANUAL==='1',transformedFit=process.env.VISUALIZATION_FIT_TRANSFORMED==='1';
const fontSymbols=process.env.VISUALIZATION_FIT_FONT_SYMBOLS==='1';
const source=process.env.VISUALIZATION_FIT_SCENE||fileURLToPath(new URL('./fixtures/scientific-egfr-mapk-illustrated.json',import.meta.url));
const binary=process.env.VISUALIZATION_NATIVE_BIN||fileURLToPath(new URL('../../../target/debug/examples/visualization_native',import.meta.url));
const directory=await mkdtemp(join(tmpdir(),'autonomics-relation-label-layout-')),origin='http://127.0.0.1:1453';
const native=spawn(binary,[join(directory,'figures.sqlite')],{stdio:['ignore','pipe','pipe']}),vite=spawn(process.execPath,['node_modules/vite/bin/vite.js','--host','127.0.0.1','--port','1453','--strictPort'],{stdio:['ignore','pipe','pipe']});
let output='',viteOutput='',browser,page;const diagnostics={requests:[],console:[]};native.stdout.on('data',b=>output+=b);native.stderr.on('data',b=>output+=b);vite.stdout.on('data',b=>viteOutput+=b);vite.stderr.on('data',b=>viteOutput+=b);
const persist=(name,value)=>writeFile(join(directory,name),JSON.stringify(value,null,2),{mode:0o600});
const until=async(fn,label)=>{const deadline=Date.now()+60000;while(!await fn()){assert.ok(Date.now()<deadline,label);await new Promise(r=>setTimeout(r,50));}};
try {
 await until(()=>{assert.equal(native.exitCode,null,output);return output.includes('VISUALIZATION_NATIVE_ADDR=');},'native readiness');await until(()=>{assert.equal(vite.exitCode,null,viteOutput);return viteOutput.includes(origin);},'Vite readiness');
 const backend=`http://${output.match(/VISUALIZATION_NATIVE_ADDR=([^\s]+)/)[1]}`;
 const call=async(method,path,body)=>{const r=await fetch(backend+path,{method,headers:{'content-type':'application/json'},...(body===undefined?{}:{body:JSON.stringify(body)}),signal:AbortSignal.timeout(30000)});const v=await r.json();assert.ok(r.ok,JSON.stringify(v));return v;};
 browser=await chromium.launch({executablePath:process.env.PDF_TEST_CHROMIUM||(existsSync('/usr/bin/chromium')?'/usr/bin/chromium':undefined),args:['--no-sandbox']});page=await browser.newPage({viewport:{width:1600,height:1000}});const errors=[];page.on('pageerror',e=>errors.push(e.message));page.on('requestfailed',r=>{if(diagnostics.requests.length<100)diagnostics.requests.push({url:r.url(),error:r.failure()?.errorText});});page.on('console',m=>{if(m.type()==='error'&&diagnostics.console.length<100)diagnostics.console.push(m.text().slice(0,1000));});
 await page.route('**/api/session',r=>r.fulfill({json:{}}));
 await page.route('**/api/v1/**',async route=>{const url=new URL(route.request().url()),p=url.pathname.replace('/api/v1','');if(p.startsWith('/visualization/'))return route.fulfill({response:await route.fetch({url:backend+url.pathname+url.search})});if(p==='/events')return new Promise(()=>{});if(p==='/state')return route.fulfill({json:{last_seq:0,agents:[],contexts:{}}});if(p==='/profiles')return route.fulfill({json:[]});if(p==='/model-config')return route.fulfill({json:{models:[],providers:[]}});if(p==='/settings')return route.fulfill({json:{}});if(p==='/storage/agents')return route.fulfill({json:[]});if(p==='/gateway/status')return route.fulfill({json:{running:true}});return route.fulfill({status:404,json:{error:`unrelated fixture ${p}`}});});
 await page.goto(origin,{waitUntil:'domcontentloaded',timeout:60000});await page.getByRole('navigation',{name:'工作台活动栏'}).getByRole('button',{name:'画布',exact:true}).click();await page.getByRole('textbox',{name:'新项目名称'}).fill('Illustration fit');await page.getByRole('button',{name:'创建项目',exact:true}).click();await page.getByRole('button',{name:'新建画布',exact:true}).click();await page.locator('[data-canvas-surface] .excalidraw:visible').waitFor();
 const project=await page.getByRole('combobox',{name:'可视化项目'}).inputValue(),figure=(await call('GET',`/api/v1/visualization/projects/${project}/figures`))[0].id,path=`/api/v1/visualization/projects/${project}/figures/${figure}`;
 const relation='受体相关复合物经明确的修饰位点及中间状态形成下游信号转导复合物，具体激酶归属尚未确定';
 const sourcePlan=process.env.RELATION_LAYOUT_PLAN?JSON.parse(await readFile(process.env.RELATION_LAYOUT_PLAN,'utf8')).document:null;
 const rows=[];
 for(const layout of ['horizontal','vertical']){
  const before=await call('GET',path),revision=before.metadata.revision;
  const plan=sourcePlan?{...Object.fromEntries(['message','audience','panels','nodes','edges','assumptions'].filter(key=>sourcePlan[key]!==undefined).map(key=>[key,sourcePlan[key]])),layout}:{message:'Conceptual relation layout fixture',audience:'Researchers',layout,nodes:[{id:'a',label:'Source'},{id:'b',label:'Intermediate'},{id:'c',label:'Endpoint'}],edges:[{from:'a',to:'b',label:relation,evidence:['Declared transition fixture']},{from:'b',to:'c',label:'enters nucleus',evidence:['Declared endpoint fixture']}]};
  const result=await call('POST','/test/visualization',{action:'sketch',project_id:project,figure_id:figure,expected_revision:revision,title:`Relations ${layout}`,document:plan});
  const candidate=await call('GET',`${path}/proposals/${result.content.id}`);
  const labelIndex=sourcePlan?plan.edges.reduce((best,edge,index)=>edge.label.length>plan.edges[best].label.length?index:best,0):0;const labelId=`edge-label-${labelIndex}`,original=plan.edges[labelIndex].label;
  const label=candidate.document.elements.find(e=>e.id===labelId);assert.ok(Number.isFinite(label.width)&&label.width>0);if(!sourcePlan){assert.ok(label.width>104);assert.ok(label.text.split('\n').length<=3);}assert.equal(label.text.replace(/\s/g,''),original.replace(/\s/g,''));assert.deepEqual(await call('GET',path),before);
  await call('POST',`${path}/proposals/${candidate.id}/preview-request`,{expected_revision:revision});let preview;
  await until(async()=>{preview=await call('GET',`${path}/proposals/${candidate.id}/preview?expected_revision=${revision}`);assert.notEqual(preview.render_status,'failed');return preview.render_status==='ready';},'actual independent relation label PNG');assert.equal(preview.document_sha256,candidate.document_sha256);await writeFile(join(directory,`${layout}.png`),Buffer.from(preview.dataURL.split(',')[1],'base64'));
  await page.getByRole('button',{name:'查看 Agent 提案',exact:true}).click();const dialog=page.getByRole('dialog',{name:'Agent 修改预览',exact:true});await dialog.getByRole('button',{name:`${candidate.summary} · v${revision}`,exact:true}).click();await dialog.getByRole('region',{name:'候选画布'}).locator('.excalidraw').waitFor();await dialog.getByRole('button',{name:'采用提案',exact:true}).click();await dialog.waitFor({state:'hidden'});
  const download=page.waitForEvent('download');await page.getByRole('button',{name:'导出 源文件',exact:true}).click();const exported=JSON.parse(await readFile(await(await download).path(),'utf8'));
  const restored=exported.elements.find(e=>e.id===labelId);assert.equal(restored.text,label.text,'native restore preserves the measured wrapping');assert.equal(restored.originalText,original);assert.equal(restored.fontSize,16);assert.equal(restored.fontFamily,9);
  assert.deepEqual(exported.elements.find(e=>e.id===`edge-${labelIndex}`).customData.evidence,plan.edges[labelIndex].evidence);await persist(`${layout}.json`,exported);rows.push({layout,width:label.width,lines:label.text.split('\n').length});
 }
 assert.deepEqual(errors,[]);await persist('result.json',{passed:true,model_used:false,plan_replayed:Boolean(sourcePlan),rows,scope:'real compiler/tool/HTTP/SQLite/Chromium render, GUI adoption and exact native text wrapping/source restoration'});console.log(`relation-label-layout: passed; artifacts=${directory}`);
} catch(error){await persist('failure.json',{error:String(error),stack:error.stack,diagnostics,native_output:output,vite_output:viteOutput});if(page)await page.screenshot({path:join(directory,'failure.png'),animations:'disabled'}).catch(()=>{});console.error(`relation-label-layout: failed; artifacts=${directory}`);throw error;}finally{if(browser)await browser.close();for(const p of [vite,native]){if(p.exitCode===null&&p.signalCode===null){p.kill();await new Promise(resolve=>p.once('exit',resolve));}}}
