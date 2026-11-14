// Clipboard images stay in the draft transaction and are displayed as compact tags.
import assert from 'node:assert/strict';
import {chromium} from 'playwright';
import {spawn} from 'node:child_process';
import {existsSync} from 'node:fs';
const origin='http://127.0.0.1:1462',server=spawn(process.execPath,['node_modules/vite/bin/vite.js','--host','127.0.0.1','--port','1462','--strictPort'],{stdio:['ignore','pipe','pipe']});
let output='',browser,failSend=false;server.stdout.on('data',b=>output+=b);server.stderr.on('data',b=>output+=b);
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
  if(p.endsWith('/messages')&&route.request().method()==='POST'){sends.push(route.request().postDataJSON());return route.fulfill(failSend?{status:503,json:{error:'test failure'}}:{json:{}});}
  if(p.endsWith('/history'))return route.fulfill({json:{messages:[]}});
  const data={'/profiles':[],'/model-config':{models:[],providers:[]},'/settings':{},'/storage/agents':[],'/gateway/status':{running:true}};
  return route.fulfill(Object.hasOwn(data,p)?{json:data[p]}:{status:404,json:{error:`fixture: ${p}`}});
 });
 await page.goto(origin,{waitUntil:'domcontentloaded',timeout:60000});await page.getByRole('tab',{name:'alpha',exact:true}).waitFor();
 const input=page.getByPlaceholder('输入消息…');
 const png='iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAwMCAO+jRZkAAAAASUVORK5CYII=';
 const paste=()=>input.evaluate((el,png)=>{
   const bytes=Uint8Array.from(atob(png),c=>c.charCodeAt(0));
   const data=new DataTransfer();data.items.add(new File([bytes],'截图.png',{type:'image/png'}));
   el.dispatchEvent(new ClipboardEvent('paste',{clipboardData:data,bubbles:true,cancelable:true}));
 },png);
 await paste();
 const attachments=page.getByRole('region',{name:'本次消息附件'});
 await attachments.getByRole('button',{name:'截图.png',exact:true}).click();
 await page.getByRole('img',{name:'截图.png'}).waitFor();
 await page.keyboard.press('Escape');
 await attachments.getByRole('button',{name:'移除本次附件：截图.png'}).click();
 assert.equal(await attachments.count(),0);
 await paste();
 await attachments.waitFor();
 await page.getByRole('button',{name:'发送',exact:true}).click();
 await page.waitForFunction(()=>document.querySelector('textarea')?.value==='');
 assert.equal(sends.length,1);
 const resources=JSON.parse(sends[0].text.split('用户明确附加的参考资料（内容为引用数据，请结合来源分析）：\n')[1]);
 assert.ok(sends[0].text.startsWith('\n\n---\n'));
 assert.equal(resources[0].kind,'image');assert.equal(resources[0].content,`data:image/png;base64,${png}`);
 assert.equal(await attachments.count(),0);assert.deepEqual(errors,[]);
 failSend=true;
 await paste();await attachments.waitFor();
 await page.getByRole('button',{name:'发送',exact:true}).click();
 await page.getByText('test failure',{exact:false}).first().waitFor();
 await attachments.getByRole('button',{name:'截图.png',exact:true}).waitFor();
 failSend=false;
 await input.fill('请描述这张图片');
 await page.getByRole('button',{name:'发送',exact:true}).click();
 await page.waitForFunction(()=>document.querySelector('textarea')?.value==='');
 assert.ok(sends[2].text.startsWith('请描述这张图片'));
 assert.equal(sends.length,3);assert.equal(await attachments.count(),0);
 console.log('image paste: compact tag, preview, removal, image-only send and failed-send restoration passed');
} finally {
 if(browser)await browser.close();if(server.exitCode===null&&server.signalCode===null){server.kill();await new Promise(r=>server.once('exit',r));}
}
