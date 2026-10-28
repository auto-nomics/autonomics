import { chromium } from 'playwright';
import assert from 'node:assert/strict';
import { spawn } from 'node:child_process';
import { once } from 'node:events';
import { existsSync } from 'node:fs';
import { createHash } from 'node:crypto';

const origin='http://127.0.0.1:1427';
const server=spawn(process.execPath,['node_modules/vite/bin/vite.js','--host','127.0.0.1','--port','1427','--strictPort'],{stdio:['ignore','pipe','pipe']});
let output='';server.stdout.on('data',value=>output+=value);server.stderr.on('data',value=>output+=value);
let browser;
const text='İ prefix genome.\n😀 gene editing evidence in the second file.\n';
const hash=createHash('sha256').update(text).digest('hex');
const article={id:'a',title:'Alpha gene editing',authors:[],identifiers:[],year:2022,keywords:['gene']};
const expression={op:'or',clauses:[{op:'phrase',field:'attachment_content',value:'gene editing'},{op:'term',value:'beta',field:'any'}]};
const facets={tags:[{value:'7',label:'gene',count:1}],creators:[{value:'9',label:'Ada Lovelace',count:1}],publication_types:[{value:'Review',label:'Review',count:1}],years:[{value:'2022',label:'2022',count:1}],fulltext_status:[{value:'indexed',label:'indexed',count:1}]};
try {
  for(let n=0;!output.includes(origin);n++){if(server.exitCode!==null||n>100)throw new Error(output);await new Promise(resolve=>setTimeout(resolve,50));}
  browser=await chromium.launch({executablePath:process.env.PDF_TEST_CHROMIUM||['/usr/bin/chromium','/opt/google/chrome/chrome'].find(existsSync),args:['--no-sandbox']});
  for(const native of [false,true]) {
    const page=await browser.newPage({viewport:{width:1440,height:1000}});
    const calls=[],errors=[];let rules=[],stale=false;
    page.on('pageerror',error=>errors.push(error.message));
    if(native) await page.addInitScript(()=>{
      const pending=new Map();window.nativeCalls=[];
      window.__TAURI_INTERNALS__={invoke:async(command,args)=>{
        window.nativeCalls.push({command,args});
        if(command==='cancel_api_request'){pending.get(args.requestId)?.abort();await fetch('/api/v1/bib/search/cancel',{method:'POST',headers:{'Content-Type':'application/json'},body:JSON.stringify({request_id:args.requestId})});return;}
        if(command==='api') {
          const controller=new AbortController();if(args.requestId)pending.set(args.requestId,controller);
          try {const response=await fetch('/api/v1'+args.path,{method:args.method,signal:controller.signal,headers:{'Content-Type':'application/json',...(args.requestId?{'x-search-request-id':args.requestId}:{})},body:args.body==null?undefined:JSON.stringify(args.body)});const body=await response.text();const data=body?JSON.parse(body):null;if(!response.ok)throw new Error(data?.error||String(response.status));return {data};}finally{pending.delete(args.requestId);}
        }
        if(command==='bib_fulltext_raw') {
          const base='/api/v1/bib/articles/'+encodeURIComponent(args.articleId);
          const response=await fetch(base+(args.attachmentKey?'/attachments/'+encodeURIComponent(args.attachmentKey)+'/raw':'/fulltext/raw')+(args.expectedHash?'?expected_hash='+encodeURIComponent(args.expectedHash):''));
          if(!response.ok)throw new Error('File changed');const bytes=new Uint8Array(await response.arrayBuffer());return {content_type:response.headers.get('content-type'),data_base64:btoa(Array.from(bytes,byte=>String.fromCharCode(byte)).join(''))};
        }
        throw new Error('Unexpected native command '+command);
      }};
    });
    await page.route(new RegExp('^'+origin.replaceAll('.','\\.')+'/?(?:\\?.*)?$'),route=>route.fulfill({contentType:'text/html',body:'<html><body></body></html>'}));
    await page.route('**/api/v1/**',async route=>{
      const request=route.request(),url=new URL(request.url()),path=url.pathname.replace(/^\/api\/v1/,''),body=request.postDataJSON();calls.push({path,query:url.search,method:request.method(),body,headers:request.headers()});
      if(path==='/bib/collections')return route.fulfill({json:{collections:[]}});
      if(path==='/bib/collections/changes')return route.fulfill({json:{}});
      if(path==='/bib/health')return route.fulfill({json:{article_count:1001}});
      if(path==='/bib/search/status')return route.fulfill({json:{engine:'tantivy',source_revision:1,indexed_revision:1,indexed_documents:1003,pending_articles:0,building:false,processed_documents:0,durable:true}});
      if(path==='/bib/search/cancel')return route.fulfill({status:202,json:{}});
      if(path==='/bib/search/parse') {
        let parsed;
        if(!body.query)parsed=null;
        else if(body.query==='alpha')parsed={op:'term',value:'alpha'};
        else if(body.query==='(fulltext:"gene editing" OR beta)')parsed=expression;
        else return route.fulfill({status:400,json:{error:'Unsupported test query '+body.query}});
        return route.fulfill({json:{expression:parsed}});
      }
      if(path==='/bib/saved-searches'&&request.method()==='GET')return route.fulfill({json:{saved_searches:structuredClone(rules)}});
      if(path==='/bib/saved-searches'&&request.method()==='POST'){const rule={id:'living',name:body.name,version:1,schema_version:1,request:body.request};rules.push(rule);return route.fulfill({status:201,json:{saved_search:structuredClone(rule)}});}
      if(path==='/bib/saved-searches/living') {
        const rule=rules[0];const expected=body?.expected_version??Number(url.searchParams.get('expected_version'));
        if(expected!==rule.version)return route.fulfill({status:409,json:{error:'Rule changed in another window; reload before editing'}});
        if(request.method()==='DELETE'){rules=[];return route.fulfill({status:204});}
        rule.name=body.name;rule.request=body.request;rule.version++;return route.fulfill({json:{saved_search:structuredClone(rule)}});
      }
      if(path==='/bib/articles')return route.fulfill({json:{articles:[article],total:1}});
      if(path==='/bib/search/v2') {
        if(body.query==='slow')await new Promise(resolve=>setTimeout(resolve,650));
        const bulk=body.query==='bulk',offset=Number(body.cursor||0);
        const articles=bulk?Array.from({length:50},(_,i)=>({id:'bulk-'+String(offset+i).padStart(4,'0'),title:'Bulk '+String(offset+i).padStart(4,'0'),authors:[],identifiers:[]})):[article];
        return route.fulfill({json:{articles,total:bulk?1000:1,index_generation:'generation-1',next_cursor:bulk&&offset+50<1000?String(offset+50):null,facets,hits:articles.map(a=>({article_id:a.id,title:a.title,score:1,matched_field:'fulltext',snippet:'😀 gene editing evidence',highlight_ranges:[{start:2,end:14}]}))}}).catch(()=>{});
      }
      if(path.endsWith('/search-evidence')) {
        if(stale)return route.fulfill({status:400,json:{error:'Search evidence expired; refresh results'}});
        const evidence=[{article_id:'a',item_key:'PARENT',attachment_key:'SECOND',attachment_hash:hash,annotation_key:null,annotation_id:null,matched_field:'fulltext',snippet:'😀 gene editing evidence',highlight_ranges:[{start:2,end:14}],matched_query:'gene editing',page:null,content_revision:'generation-1',location_status:'text_search'}];
        return route.fulfill({json:{evidence,total:1,next_cursor:null,index_generation:'generation-1'}});
      }
      if(path==='/bib/articles/a')return route.fulfill({json:{article,fulltext:{file_format:'txt',attachment_key:'FIRST'},annotations:[],attachments:[{key:'FIRST',link_mode:0,content_type:'text/plain',title:'Preferred file',path:'first.txt'},{key:'SECOND',link_mode:0,content_type:'text/plain',title:'Matching file',path:'second.txt',storage_hash:hash}]}});
      if(path==='/bib/articles/a/attachments/SECOND/raw') {
        assert.equal(url.searchParams.get('expected_hash'),hash);return route.fulfill({contentType:'text/plain; charset=utf-8',body:text});
      }
      if(path.endsWith('/annotations'))return route.fulfill({json:{annotations:[]}});
      if(path.startsWith('/bib/articles/bulk-'))return route.fulfill({json:{article:{id:path.split('/').at(-1),title:'Bulk selection',authors:[],identifiers:[]},fulltext:null,annotations:[]}});
      throw new Error('Unhandled request '+request.method()+' '+path);
    });
    const mount=()=>page.evaluate(async()=>{
      await import('/src/app.css');const {default:Refresh}=await import('/@react-refresh');Refresh.injectIntoGlobalHook(window);window.$RefreshReg$=()=>{};window.$RefreshSig$=()=>type=>type;window.__vite_plugin_react_preamble_installed__=true;
      const {mountBibliography}=await import('/tests/bibliography-menu-mount.tsx');mountBibliography(document.body);
    });
    const search=()=>page.getByRole('textbox',{name:'Search bibliography'});
    const resultList=()=>page.getByRole('list',{name:'Literature results'});
    await page.goto(origin);await mount();await page.getByRole('button',{name:/Alpha gene editing/}).waitFor();
    await search().fill('alpha');await search().press('Enter');await page.locator('mark').first().waitFor();
    await page.getByLabel('Quick search fields').selectOption('title');
    await page.getByRole('button',{name:'Advanced',exact:true}).click();const dialog=page.getByRole('dialog',{name:'Advanced search'});
    await dialog.getByLabel('Condition operator').first().selectOption('phrase');await dialog.getByLabel('Condition field').selectOption('attachment_content');await dialog.getByLabel('Condition value').fill('gene editing');
    await dialog.getByLabel('Condition operator').first().selectOption('or');await dialog.getByRole('button',{name:'Add condition',exact:true}).click();await dialog.getByLabel('Condition value').nth(1).fill('beta');
    await dialog.getByRole('button',{name:'Apply search',exact:true}).click();await dialog.waitFor({state:'detached'});
    await page.getByRole('button',{name:'Filters',exact:true}).click();await page.getByLabel('Filter year from').fill('2020');
    await page.getByRole('group',{name:'Tags',exact:true}).getByRole('checkbox',{name:/gene/}).check();
    await page.getByRole('group',{name:'Authors',exact:true}).getByRole('checkbox',{name:/Ada Lovelace/}).check();
    await page.getByRole('group',{name:'Publication types',exact:true}).getByRole('checkbox',{name:/Review/}).check();
    await page.getByRole('checkbox',{name:'Same attachment',exact:true}).check();
    await page.getByRole('button',{name:'Save current search',exact:true}).click();await page.getByLabel('Search name').fill('Living review');await page.getByRole('button',{name:'Save rule',exact:true}).click();
    await page.getByRole('button',{name:/⌕ Living review/}).waitFor();assert.deepEqual(rules[0].request.filters.tag_ids,[7]);assert.equal(rules[0].request.scope.require_same_child,true);assert.equal(rules[0].request.expression.op,'or');
    const savedQuery=await search().inputValue();await page.reload();await mount();await page.getByRole('button',{name:/⌕ Living review/}).waitFor();
    assert.equal(await search().inputValue(),savedQuery);assert.equal(await page.getByLabel('Filter year from').inputValue(),'2020');assert.equal(await page.getByLabel('Quick search fields').inputValue(),'title');
    await page.getByRole('button',{name:'Update saved search',exact:true}).click();rules[0].version++;
    await page.getByRole('button',{name:'Save rule',exact:true}).click();await page.getByRole('dialog').getByRole('alert').filter({hasText:'changed in another window'}).waitFor();
    await page.getByRole('dialog').getByRole('button',{name:'Cancel',exact:true}).click();
    await page.getByRole('button',{name:/Alpha gene editing/}).click();await page.getByRole('button',{name:'Open evidence',exact:true}).waitFor();
    await page.getByRole('checkbox',{name:'Select Alpha gene editing',exact:true}).check();
    const viewport=page.getByLabel('Literature results viewport'),scrollBefore=await viewport.evaluate(node=>node.scrollTop);
    await page.getByRole('button',{name:'Open evidence',exact:true}).click();const reader=page.locator('.zotero-pdf-reader[data-reader-kind=txt]');await reader.waitFor();await reader.getByRole('button',{name:/Match 1/}).waitFor();
    await page.keyboard.press('Control+f');assert.equal(await reader.locator('[data-input=search]').evaluate(node=>node===document.activeElement),true);
    const ranges=await page.evaluate(async()=>{const {findTextRanges}=await import('/src/features/reader/reader-target.ts');const source='İ before genome and gene';return findTextRanges(source,'gene',true).map(r=>source.slice(r.start,r.end));});assert.deepEqual(ranges,['gene']);
    await reader.locator('[data-action=close]').click();await reader.waitFor({state:'detached'});assert.equal(await viewport.evaluate(node=>node.scrollTop),scrollBefore);assert.equal(await page.getByRole('checkbox',{name:'Select Alpha gene editing',exact:true}).isChecked(),true);
    assert.equal(calls.some(call=>call.path.endsWith('/FIRST/raw')),false);assert.equal(calls.some(call=>call.path.endsWith('/SECOND/annotations')),true);
    if(native) assert.equal(await page.evaluate(()=>window.nativeCalls.some(call=>call.command==='bib_fulltext_raw'&&call.args.attachmentKey==='SECOND')),true);
    stale=true;const rawCount=calls.filter(call=>call.path.endsWith('/raw')).length;await page.getByRole('button',{name:'Open evidence',exact:true}).click();await page.getByRole('alert').filter({hasText:'Search evidence expired'}).first().waitFor();assert.equal(calls.filter(call=>call.path.endsWith('/raw')).length,rawCount);stale=false;
    await page.getByRole('button',{name:'Delete saved search Living review',exact:true}).click();await page.getByRole('button',{name:'Delete rule',exact:true}).click();await page.getByRole('button',{name:/⌕ Living review/}).waitFor({state:'detached'});assert.equal(await page.getByRole('button',{name:/Alpha gene editing/}).count(),1);
    await page.locator('summary').filter({hasText:/^Recent$/}).click();assert.ok(await page.locator('details[open]').getByRole('button',{name:savedQuery,exact:true}).count());await page.locator('summary').filter({hasText:/^Recent$/}).click();
    await page.locator('summary').filter({hasText:/^View$/}).click();await page.getByRole('checkbox',{name:'Journal column',exact:true}).check();await page.getByRole('checkbox',{name:'Show match snippets',exact:true}).uncheck();await page.getByRole('checkbox',{name:'Show match snippets',exact:true}).check();await page.locator('summary').filter({hasText:/^View$/}).click();
    await page.getByRole('button',{name:'Clear search',exact:true}).click();await search().fill('slow');await Promise.all([page.waitForRequest(req=>req.url().endsWith('/search/v2')&&req.postDataJSON()?.query==='slow'),search().press('Enter')]);
    await search().fill('alpha');await search().press('Enter');await page.waitForTimeout(750);
    const slow=calls.findLast(call=>call.path==='/bib/search/v2'&&call.body.query==='slow');assert.ok(calls.some(call=>call.path==='/bib/search/cancel'&&call.body.request_id===slow.headers['x-search-request-id']));
    await search().fill('bulk');await search().press('Enter');await page.waitForFunction(()=>document.querySelector('[aria-label="Literature results"]')?.dataset.loadedCount==='50');
    await page.getByRole('checkbox',{name:'Select Bulk 0000',exact:true}).check();await search().focus();
    for(let target=100;target<=300;target+=50){await viewport.evaluate(node=>{node.scrollTop=node.scrollHeight;});await page.waitForFunction(target=>Number(document.querySelector('[aria-label="Literature results"]')?.dataset.loadedCount)>=target,target);await viewport.evaluate(()=>new Promise(resolve=>requestAnimationFrame(()=>requestAnimationFrame(resolve))));}
    assert.ok(await resultList().locator('[data-article-id]').count()<80);assert.equal(await resultList().locator('[data-article-id="bulk-0000"]').count(),0);
    await viewport.evaluate(node=>{node.scrollTop=0;});await page.getByRole('checkbox',{name:'Select Bulk 0000',exact:true}).waitFor();assert.equal(await page.getByRole('checkbox',{name:'Select Bulk 0000',exact:true}).isChecked(),true);
    await resultList().locator('[data-article-id="bulk-0000"] [data-bib-row-primary]').focus();await page.keyboard.press('ArrowDown');await page.waitForFunction(()=>document.activeElement?.closest('[data-article-id]')?.dataset.articleId==='bulk-0001');await page.keyboard.press('Control+Space');assert.equal(await page.getByRole('checkbox',{name:'Select Bulk 0001',exact:true}).isChecked(),true);
    assert.equal(await resultList().locator('[data-article-id="bulk-0001"]').getAttribute('aria-posinset'),'2');
    await page.screenshot({path:`/tmp/bib-workflow-${native?'native':'web'}.png`});assert.deepEqual(errors,[]);
    console.log(`${native?'Tauri bridge':'Browser'}: advanced/save/conflict/URL/evidence/cancellation/virtual selection passed`);await page.close();
  }
} finally {await browser?.close();server.kill('SIGTERM');await once(server,'exit');}
