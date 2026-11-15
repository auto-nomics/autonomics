import {chromium} from 'playwright';
import assert from 'node:assert/strict';
import {spawn} from 'node:child_process';
import {once} from 'node:events';
import {existsSync} from 'node:fs';
import {mkdtemp,mkdir,writeFile,rm} from 'node:fs/promises';
import {tmpdir} from 'node:os';
import {join} from 'node:path';
import {createHash} from 'node:crypto';
const origin='http://127.0.0.1:1448';
const server=spawn(process.execPath,['node_modules/vite/bin/vite.js','--host','127.0.0.1','--port','1448','--strictPort'],{stdio:['ignore','pipe','pipe']});
let output='';server.stdout.on('data',value=>output+=value);server.stderr.on('data',value=>output+=value);
const folder=await mkdtemp(join(tmpdir(),'zotero-bundle-'));const file='PDF original fixture';
await mkdir(join(folder,'files','PAPER'),{recursive:true});await writeFile(join(folder,'files','PAPER','paper.pdf'),file);
const contents=JSON.stringify({items:[{key:'PAPER',itemType:'journalArticle',title:'Sample',collections:['C'],notes:[{key:'N',note:'<p>Hello &amp; science</p><script>bad()</script>'}],attachments:[{key:'A',path:'files/PAPER/paper.pdf',title:'Paper PDF'}]}],collections:{C:{name:'Research'}}});
let browser;
try {
  for(let n=0;!output.includes(origin);n++){if(server.exitCode!==null||n>100)throw new Error(output);await new Promise(resolve=>setTimeout(resolve,50));}
  browser=await chromium.launch({executablePath:process.env.PDF_TEST_CHROMIUM||['/usr/bin/chromium','/opt/google/chrome/chrome'].find(existsSync),args:['--no-sandbox']});
  const page=await browser.newPage({viewport:{width:1200,height:900}});const errors=[],notes=[],collections=[],attachments=[];let imported=false,uploads=0;
  page.on('pageerror',error=>errors.push(error.message));
  await page.addInitScript(() => {
    window.uploadedFiles = [];
    const original = window.fetch;
    window.fetch = async (url, options) => {
      if (options?.body instanceof FormData && String(url).endsWith('/attachments')) {
        const file = options.body.get('file');
        window.uploadedFiles.push({ content: await file.text(), title: options.body.get('title'), preferred: options.body.get('preferred') });
      }
      return original(url, options);
    };
  });
  await page.route(`${origin}/`,route=>route.fulfill({contentType:'text/html',body:'<html><body></body></html>'}));
  await page.route('**/api/v1/**',async route=>{
    const req=route.request(),path=new URL(req.url()).pathname.replace('/api/v1','');
    const body=req.headers()['content-type']?.includes('application/json')?req.postDataJSON():null;
    if(path==='/bib/import/preview')return route.fulfill({json:{records:[{index:0,article:{id:'a',title:'Sample'},duplicate_id:imported?'a':null,warnings:['Non-author contributor roles are not mapped']}]}});
    if(path==='/bib/import/commit'){const imported_ids=imported?[]:['a'];imported=true;return route.fulfill({json:{imported_ids,skipped_ids:[]}});}
    if(path==='/bib/collections'&&req.method()==='GET')return route.fulfill({json:{collections}});
    if(path==='/bib/collections'&&req.method()==='POST'){const collection={...body,id:'c'};collections.push(collection);return route.fulfill({json:{collection}});}
    if(path==='/bib/articles/a')return route.fulfill({json:{article:{id:'a',title:'Sample'},annotations:notes,attachments,fulltext:null}});
    if(path==='/bib/articles/a/annotations'){const annotation={...body,id:'n',tags:[]};notes.push(annotation);return route.fulfill({json:{annotation}});}
    if(path==='/bib/annotations/n'){notes[0].tags=body.tags;return route.fulfill({json:{annotation:notes[0]}});}
    if(path==='/bib/articles/a/attachments'){
      uploads++;attachments.push({key:'attachment',storage_hash:createHash('sha256').update(file).digest('hex')});return route.fulfill({json:{attachment:attachments[0]}});
    }
    if(path==='/bib/collections/c/articles')return route.fulfill({json:{ok:true}});
    return route.fulfill({status:404,json:{error:`Unexpected ${path}`}});
  });
  await page.goto(origin);await page.evaluate(async()=>{
    await import('/src/app.css');const{default:RefreshRuntime}=await import('/@react-refresh');RefreshRuntime.injectIntoGlobalHook(window);window.$RefreshReg$=()=>{};window.$RefreshSig$=()=>type=>type;window.__vite_plugin_react_preamble_installed__=true;
    const{mountZoteroBundle}=await import('/tests/zotero-bundle-mount.tsx');mountZoteroBundle(document.body);
  });
  await page.getByRole('button',{name:'Migrate Zotero library',exact:true}).click();
  const dialog=page.getByRole('dialog',{name:'Migrate Zotero library',exact:true});
  await dialog.getByLabel('Zotero JSON export',{exact:true}).setInputFiles({name:'export.json',mimeType:'application/json',buffer:Buffer.from(contents)});
  await dialog.getByLabel('Zotero export folder',{exact:true}).setInputFiles(folder);
  await dialog.getByRole('button',{name:'Preview migration',exact:true}).click();
  await dialog.getByText('1 of 1 referenced attachment files matched.',{exact:true}).waitFor();
  await dialog.getByText('Sample: Non-author contributor roles are not mapped',{exact:true}).waitFor();
  await dialog.getByRole('button',{name:'Import metadata, notes and files',exact:true}).click();
  await dialog.getByRole('status').filter({hasText:'Imported 1 records, 1 collections, 1 notes and 1 files.'}).waitFor();
  await dialog.getByRole('status').getByText('Sample: Non-author contributor roles are not mapped',{exact:true}).waitFor();
  assert.equal(notes[0].content,'Hello & science');assert.deepEqual(await page.evaluate(() => window.uploadedFiles), [{content:file,title:'Paper PDF',preferred:'true'}]);assert.equal(uploads,1);assert.equal(collections.length,1);
  await dialog.getByRole('button',{name:'Preview migration',exact:true}).click();
  await dialog.getByRole('button',{name:'Import metadata, notes and files',exact:true}).click();
  await dialog.getByRole('status').filter({hasText:'Imported 0 records, 0 collections, 0 notes and 0 files.'}).waitFor();
  assert.equal(notes.length,1);assert.equal(uploads,1);assert.deepEqual(errors,[]);
  console.log('PASS Zotero bundle GUI: file/directory selection, preview, metadata + collection + plain notes + original attachment upload, retry without duplicates');
} finally {await browser?.close();server.kill();if(server.exitCode===null)await once(server,'exit');await rm(folder,{recursive:true,force:true});}
