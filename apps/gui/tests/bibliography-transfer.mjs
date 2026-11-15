import { chromium } from 'playwright';
import assert from 'node:assert/strict';
import { spawn } from 'node:child_process';
import { once } from 'node:events';
import { existsSync } from 'node:fs';
const origin='http://127.0.0.1:1439';
const server=spawn(process.execPath,['node_modules/vite/bin/vite.js','--host','127.0.0.1','--port','1439','--strictPort'],{stdio:['ignore','pipe','pipe']});
let output='';server.stdout.on('data',value=>output+=value);server.stderr.on('data',value=>output+=value);let browser;
try{
 for(let i=0;!output.includes(origin);i++){if(i>200||server.exitCode!==null)throw new Error(output);await new Promise(resolve=>setTimeout(resolve,50));}
 browser=await chromium.launch({executablePath:process.env.PDF_TEST_CHROMIUM||['/usr/bin/chromium','/opt/google/chrome/chrome'].find(existsSync),args:['--no-sandbox']});
 const page=await browser.newPage();const errors=[],requests=[];let merged=false;page.on('pageerror',error=>errors.push(error.message));page.on('dialog',dialog=>dialog.accept());
 await page.route(`${origin}/`,route=>route.fulfill({contentType:'text/html',body:'<html><body></body></html>'}));
 await page.route('**/api/v1/**',route=>{
  const path=new URL(route.request().url()).pathname;const body=route.request().postDataJSON();requests.push({path,body});
  if(path.endsWith('/import/preview'))return route.fulfill({json:{records:[{index:0,article:{id:'a',title:'New paper'},warnings:[],duplicate_id:null,duplicate_in_file:null},{index:1,article:{id:'b',title:'Existing paper'},warnings:[],duplicate_id:'b',duplicate_in_file:null}]}});
  if(path.endsWith('/import/commit'))return route.fulfill({json:{imported_ids:['a'],skipped_ids:[]}});
  if(path.endsWith('/citations/render'))return route.fulfill({json:{export:'@article{smith2024,...}',items:[{id:'a',type:'article-journal',title:'Genomic evidence',author:[{family:'Smith',given:'Jane'}],issued:{'date-parts':[[2024]]},'container-title':'Test Journal',volume:'12',page:'1-9',DOI:'10.1234/example'}]}});
  if(path.endsWith('/duplicates/merge')){merged=true;return route.fulfill({json:{article:{id:'a',title:'Primary'}}});}
  return route.fulfill({json:{groups:merged?[]:[{reason:'title:study',articles:[{id:'a',title:'Primary',abstract_text:'Primary abstract'},{id:'b',title:'Secondary',abstract_text:'Secondary abstract',extra_fields:{publisher:'Publisher'}}],revisions:{a:'rev-a',b:'rev-b'}}],total:merged?0:1,truncated:false}});
 });
 await page.goto(origin);
 await page.evaluate(async()=>{await import('/src/app.css');const {default:refresh}=await import('/@react-refresh');refresh.injectIntoGlobalHook(window);window.$RefreshReg$=()=>{};window.$RefreshSig$=()=>type=>type;window.__vite_plugin_react_preamble_installed__=true;const {mountTransfer}=await import('/tests/bibliography-transfer-mount.tsx');window.mountTransfer=mountTransfer;await window.mountTransfer('import');});
 await page.getByRole('textbox',{name:'Import contents'}).fill('@article{a,title={New paper}}');
 await page.getByRole('button',{name:'Preview import',exact:true}).click();
 await page.getByRole('checkbox',{name:'Import New paper',exact:true}).waitFor();
 assert.ok(await page.getByRole('checkbox',{name:'Import Existing paper',exact:true}).isDisabled());
 await page.getByRole('button',{name:'Import 1 selected',exact:true}).click();
 await page.waitForFunction(()=>window.imported===true);
 assert.deepEqual(requests.find(row=>row.path.endsWith('/import/commit')).body.selected_indices,[0]);
 await page.evaluate(()=>window.mountTransfer('export'));
 await page.waitForFunction(()=>document.querySelector('[aria-label="Bibliography preview"]')?.value.includes('Smith'));
 const apa=await page.getByRole('textbox',{name:'Bibliography preview'}).inputValue();assert.match(apa,/2024/);assert.match(apa,/Genomic evidence/);assert.match(apa,/Test Journal/);
 await page.getByRole('combobox',{name:'Bibliography format'}).selectOption('vancouver');
 await page.waitForFunction(()=>document.querySelector('[aria-label="Bibliography preview"]')?.value.startsWith('1.'));
 assert.notEqual(await page.getByRole('textbox',{name:'Bibliography preview'}).inputValue(),apa);
 await page.getByRole('combobox',{name:'Bibliography format'}).selectOption('gb7714');
 await page.waitForFunction(()=>document.querySelector('[aria-label="Bibliography preview"]')?.value.includes('[1]'));
 assert.match(await page.getByRole('textbox',{name:'Bibliography preview'}).inputValue(),/\[J/);
 await page.getByRole('combobox',{name:'Bibliography format'}).selectOption('bibtex');
 await page.waitForFunction(()=>document.querySelector('[aria-label="Bibliography preview"]')?.value.startsWith('@article'));
 const download=page.waitForEvent('download');await page.getByRole('button',{name:'Download',exact:true}).click();assert.equal((await download).suggestedFilename(),'references.bib');
 await page.evaluate(async()=>{const {mountDuplicates}=await import('/tests/bibliography-transfer-mount.tsx');await mountDuplicates();});
 await page.getByRole('combobox',{name:'Merged Abstract',exact:true}).selectOption('b');
 await page.getByRole('combobox',{name:'Merged Type-specific fields',exact:true}).selectOption('b');
 await page.getByRole('button',{name:'Merge into selected record',exact:true}).click();
 await page.waitForFunction(()=>window.merged===true);
 const mergeRequest=requests.find(row=>row.path.endsWith('/duplicates/merge')).body;
 assert.equal(mergeRequest.target_id,'a');assert.equal(mergeRequest.metadata.title,'Primary');assert.equal(mergeRequest.metadata.abstract_text,'Secondary abstract');assert.equal(mergeRequest.metadata.extra_fields.publisher,'Publisher');
 assert.deepEqual(errors,[]);console.log('PASS bibliography transfer: preview selection, duplicate skip, import callback, real APA/NLM/GB-T CSL rendering, BibTeX download, per-field duplicate review');
}finally{await browser?.close();server.kill();if(server.exitCode===null)await once(server,'exit');}
