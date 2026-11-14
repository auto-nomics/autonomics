import assert from 'node:assert/strict';
import { chromium } from 'playwright';
import { spawn } from 'node:child_process';
import { existsSync, mkdirSync } from 'node:fs';
const server=spawn('node',['node_modules/vite/bin/vite.js','--host','127.0.0.1','--port','5197','--strictPort'],{stdio:'pipe'});
let output='';server.stdout.on('data',b=>output+=b);server.stderr.on('data',b=>output+=b);
let browser;
try{
 for(let i=0;!output.includes('5197');i++){if(i>200||server.exitCode!==null)throw new Error(output);await new Promise(r=>setTimeout(r,50));}
 browser=await chromium.launch({executablePath:existsSync('/usr/bin/chromium')?'/usr/bin/chromium':undefined,args:['--no-sandbox']});
 const page=await browser.newPage();page.setDefaultTimeout(10000);page.on('pageerror',e=>console.error(e.message));await page.route('**/runtime-test',r=>r.fulfill({contentType:'text/html',body:'<html><body>runtime</body></html>'}));await page.goto('http://127.0.0.1:5197/runtime-test');
 const result=await page.evaluate(async()=>{
  const {renderCodeDrawing,DEFAULT_DRAWING}=await import('/src/features/canvas/canvas-code-runtime.ts');
  const image=await renderCodeDrawing(DEFAULT_DRAWING);const img=new Image();img.src=image;await img.decode();
  const canvas=document.createElement('canvas');canvas.width=img.width;canvas.height=img.height;const ctx=canvas.getContext('2d');ctx.drawImage(img,0,0);
  const alpha=ctx.getImageData(0,0,480,320).data.filter((_,i)=>i%4===3).some(a=>a>0);
  const fail=async(code,timeout)=>{try{await renderCodeDrawing({...DEFAULT_DRAWING,code},timeout);return 'unexpected success';}catch(e){return String(e);}};
  const syntax=await fail('function draw( {');const timeout=await fail('function draw(){while(true){}}',300);
  const network=await fail('async function draw(){await fetch("https://example.com")}');const dom=await fail('function draw(){document.body.innerHTML="bad"}');
  const recovery=await renderCodeDrawing(DEFAULT_DRAWING);
  return {width:img.width,height:img.height,alpha,syntax,timeout,network,dom,recovery:recovery.startsWith('data:image/png'),frames:document.querySelectorAll('iframe[sandbox]').length};
 });
 assert.equal(result.width,480);assert.equal(result.height,320);assert.equal(result.alpha,true);assert.match(result.syntax,/SyntaxError/);assert.match(result.timeout,/超时/);assert.match(result.network,/fetch|Failed/i);assert.match(result.dom,/document/);assert.equal(result.recovery,true);assert.equal(result.frames,0);
 console.log('code drawing runtime passed',result);
 await page.route('**/code-harness',r=>r.fulfill({contentType:'text/html',body:'<html><body><div id="root"></div><script type="module">import RefreshRuntime from "/@react-refresh";RefreshRuntime.injectIntoGlobalHook(window);window.$RefreshReg$=()=>{};window.$RefreshSig$=()=>type=>type;window.__vite_plugin_react_preamble_installed__=true;</script><script type="module" src="/tests/helpers/code-drawing-harness.tsx"></script></body></html>'}));
 await page.goto('http://127.0.0.1:5197/code-harness');
 await page.getByRole('button',{name:'代码绘图',exact:true}).click();
 await page.getByRole('button',{name:'运行并预览',exact:true}).click();
 await page.getByAltText('代码绘图预览').waitFor();
 mkdirSync('../../target/code-drawing',{recursive:true});await page.screenshot({path:'../../target/code-drawing/editor.png'});
 await page.getByRole('button',{name:'插入画布',exact:true}).click();
 let scene=await page.evaluate(()=>window.codeHarness.get());assert.equal(scene.elements.length,1);assert.equal(scene.elements[0].type,'image');assert.equal(scene.elements[0].customData.codeDrawing.version,1);
 const original=scene.elements[0];
 await page.evaluate(()=>window.codeHarness.reload());
 await page.getByRole('button',{name:'代码绘图',exact:true}).click();
 await page.getByRole('heading',{name:'编辑代码元素',exact:true}).waitFor();
 assert.match(await page.getByLabel('绘制函数',{exact:true}).inputValue(),/function draw/);
 await page.getByLabel('绘图参数',{exact:true}).fill('严禁无效JSON');
 await page.getByRole('button',{name:'运行并预览',exact:true}).click();
 await page.getByRole('alert').waitFor();assert.equal(await page.getByRole('button',{name:'更新元素',exact:true}).isDisabled(),true);
 await page.getByLabel('绘图参数',{exact:true}).fill('{"label":"Edited","color":"#ff0000","count":5}');
 await page.getByRole('button',{name:'运行并预览',exact:true}).click();await page.getByAltText('代码绘图预览').waitFor();
 await page.getByLabel('绘图宽度',{exact:true}).fill('500');assert.equal(await page.getByRole('button',{name:'更新元素',exact:true}).isDisabled(),true);
 await page.getByRole('button',{name:'运行并预览',exact:true}).click();await page.getByAltText('代码绘图预览').waitFor();
 await page.getByRole('button',{name:'更新元素',exact:true}).click();
 scene=await page.evaluate(()=>window.codeHarness.get());assert.equal(scene.elements.length,1);assert.equal(scene.elements[0].id,original.id);assert.equal(scene.elements[0].x,original.x);assert.equal(scene.elements[0].width,original.width);assert.notEqual(scene.elements[0].fileId,original.fileId);assert.equal(scene.elements[0].customData.codeDrawing.params.label,'Edited');
 await page.evaluate(()=>window.codeHarness.reload());assert.ok(await page.evaluate(()=>window.codeHarness.export())>0);
 console.log('code drawing UI: insert, invalid JSON, stale preview, edit, native save/reload and PNG export passed');
}finally{await browser?.close();server.kill();}
