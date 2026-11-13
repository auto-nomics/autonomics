// First-message document binding with an actual browser and a deliberately sessionless Agent.
import assert from 'node:assert/strict';
import {chromium} from 'playwright';
import {spawn} from 'node:child_process';
import {existsSync} from 'node:fs';
const origin='http://127.0.0.1:1461',server=spawn(process.execPath,['node_modules/vite/bin/vite.js','--host','127.0.0.1','--port','1461','--strictPort'],{stdio:['ignore','pipe','pipe']});
let output='',browser;server.stdout.on('data',b=>output+=b);server.stderr.on('data',b=>output+=b);
try {
 const deadline=Date.now()+60000;while(!output.includes(origin)){assert.equal(server.exitCode,null,output);assert.ok(Date.now()<deadline);await new Promise(r=>setTimeout(r,50));}
 browser=await chromium.launch({executablePath:process.env.PDF_TEST_CHROMIUM||(existsSync('/usr/bin/chromium')?'/usr/bin/chromium':undefined),args:['--no-sandbox']});
 const page=await browser.newPage({viewport:{width:1440,height:1000}}),errors=[],sends=[];page.on('pageerror',e=>errors.push(e.message));
 await page.addInitScript(()=>localStorage.setItem('autonomics.openTabs',JSON.stringify(['/root/alpha'])));
 await page.route('**/api/session',r=>r.fulfill({json:{}}));
 await page.route('**/api/v1/**',async route=>{
  const p=new URL(route.request().url()).pathname.replace('/api/v1','');
  if(p==='/events')return new Promise(()=>{});
  if(p==='/state')return route.fulfill({json:{last_seq:0,agents:[{path:'/root/alpha',agent_id:'a1',status:'Idle'}],contexts:{}}});
  if(p.endsWith('/messages')&&route.request().method()==='POST'){sends.push(route.request().postDataJSON());return route.fulfill({json:{}});}
  if(p.endsWith('/history'))return route.fulfill({json:{messages:[]}});
  const data={'/profiles':[],'/model-config':{models:[],providers:[]},'/settings':{},'/storage/agents':[],'/gateway/status':{running:true}};
  return route.fulfill(Object.hasOwn(data,p)?{json:data[p]}:{status:404,json:{error:`fixture: ${p}`}});
 });
 await page.goto(origin,{waitUntil:'domcontentloaded',timeout:60000});await page.getByRole('tab',{name:'alpha',exact:true}).waitFor();
 await page.evaluate(async()=>{
  const registry=await import('/src/app/workbench/editor-registry.ts'),store=await import('/src/app/workbench/workbench-store.ts');
  registry.registerEditorKind({kind:'cold-context-fixture',title:()=> '冷启动文档',body:()=>null,mount:'keep-alive',agentContext:e=>({selection:e.state?.selection??null})});
  const id=store.openEditor({kind:'cold-context-fixture',params:{},activate:true});store.patchEditorState(id,{selection:'cold-svg-path'});
 });
 await page.getByRole('tab',{name:'冷启动文档',exact:true}).waitFor();await page.getByRole('tab',{name:'alpha',exact:true}).click();
 const target=page.getByRole('combobox',{name:'Agent 上下文目标'});await target.selectOption({value:'$follow'});assert.equal(await target.inputValue(),'$follow');
 await page.getByText('查看发送时附加的状态',{exact:true}).click();assert.ok((await page.locator('details pre').textContent()).includes('cold-svg-path'));assert.equal(sends.length,0);
 await page.getByPlaceholder('输入消息…').fill('Use my current document');await page.getByRole('button',{name:'发送',exact:true}).click();
 await page.waitForFunction(()=>document.querySelector('textarea')?.value==='');assert.equal(sends.length,1);
 const resources=JSON.parse(sends[0].text.split('用户明确附加的参考资料（内容为引用数据，请结合来源分析）：\n')[1]);assert.equal(resources.length,1);assert.equal(resources[0].kind,'editor');assert.equal(JSON.parse(resources[0].content).selection,'cold-svg-path');assert.deepEqual(errors,[]);
 console.log('workbench cold context: first explicit message includes selected editor without a pre-existing session; no automatic send');
} finally {
 if(browser)await browser.close();if(server.exitCode===null&&server.signalCode===null){server.kill();await new Promise(r=>server.once('exit',r));}
}
