// Browser rendering contract for the production document diff panel; no model or API fixture.
import assert from 'node:assert/strict';
import {chromium} from 'playwright';
import {spawn} from 'node:child_process';
import {existsSync} from 'node:fs';
import {mkdtemp,writeFile} from 'node:fs/promises';
import {tmpdir} from 'node:os';
import {join} from 'node:path';
const directory=await mkdtemp(join(tmpdir(),'autonomics-document-changes-'));
const origin='http://127.0.0.1:1462';
const vite=spawn(process.execPath,['node_modules/vite/bin/vite.js','--host','127.0.0.1','--port','1462','--strictPort'],{stdio:['ignore','pipe','pipe']});
let output='',browser;vite.stdout.on('data',b=>output+=b);vite.stderr.on('data',b=>output+=b);
try {
  const deadline=Date.now()+30000;
  while(!output.includes(origin)){assert.equal(vite.exitCode,null,output);assert.ok(Date.now()<deadline,'Vite readiness');await new Promise(r=>setTimeout(r,50));}
  browser=await chromium.launch({executablePath:process.env.PDF_TEST_CHROMIUM||(existsSync('/usr/bin/chromium')?'/usr/bin/chromium':undefined),args:['--no-sandbox']});
  const page=await browser.newPage();const errors=[];page.on('pageerror',e=>errors.push(e.message));
  await page.route('**/document-review-test',route=>route.fulfill({contentType:'text/html',body:'<!doctype html><html><body><div id="root"></div><script type="module" src="/@vite/client"></script></body></html>'}));
  await page.goto(`${origin}/document-review-test`);
  await page.evaluate(async()=>{
    const {default:runtime}=await import('/@react-refresh');runtime.injectIntoGlobalHook(window);
    window.$RefreshReg$=()=>{};window.$RefreshSig$=()=>type=>type;window.__vite_plugin_react_preamble_installed__=true;
  });
  async function render(before,after){
    return page.evaluate(async({before,after})=>{
      const reactModule=await import('/node_modules/.vite/deps/react.js');
      const React=reactModule.default??reactModule;
      const client=await import('/node_modules/.vite/deps/react-dom_client.js');
      const createRoot=client.createRoot??client.default.createRoot;
      const {proposalDiff}=await import('/src/features/canvas/canvas-proposal-diff.ts');
      const {CanvasDocumentChanges}=await import('/src/features/canvas/canvas-document-changes.tsx');
      const diff=proposalDiff(before,after);
      window.reviewRoot??=createRoot(document.getElementById('root'));
      window.reviewRoot.render(React.createElement(CanvasDocumentChanges,{fields:diff.documentFields}));
      return {changed:diff.changed,attributes:diff.documentFieldCount};
    },{before,after});
  }
  const before={elements:[],metadata:{source:'source-A'}};
  const after={elements:[],metadata:{source:'source-B'},previewDataURL:'PRIVATE-BYTES',note:'<img src=x onerror="window.injected=true">'};
  assert.deepEqual(await render(before,after),{changed:0,attributes:3});
  const panel=page.getByRole('region',{name:'文档属性变化'});await panel.waitFor();
  const text=await panel.textContent();assert.ok(text.includes('source-A')&&text.includes('source-B'));assert.ok(text.includes('（内嵌数据）'));assert.ok(!text.includes('PRIVATE-BYTES'));
  assert.equal(await panel.locator('tbody tr').count(),3);assert.equal(await panel.locator('img').count(),0);assert.equal(await page.evaluate(()=>window.injected),undefined);
  await page.screenshot({path:join(directory,'document-attributes.png')});
  await render(before,before);await panel.waitFor({state:'detached'});assert.deepEqual(errors,[]);
  await writeFile(join(directory,'result.json'),JSON.stringify({passed:true,model_used:false,scope:'production diff function and document attribute panel rendered in browser; root-only change, byte redaction, escaped text and unchanged state'},null,2));
  console.log(`document-changes: passed; artifacts=${directory}`);
} finally {await browser?.close();vite.kill('SIGTERM');}
