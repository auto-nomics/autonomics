// Local GUI fixtures for browser fetch and simulated desktop IPC, not native qualification.
import { chromium } from 'playwright';
import { spawn } from 'node:child_process';
import { once } from 'node:events';
import { existsSync } from 'node:fs';
import assert from 'node:assert/strict';
const vite=spawn(process.execPath,['node_modules/vite/bin/vite.js','--host','127.0.0.1','--port','0'],{stdio:['ignore','pipe','pipe']});
let log='',browser;
vite.stdout.on('data',v=>log+=v);vite.stderr.on('data',v=>log+=v);
try {
  for(let n=0;!log.match(/http:\/\/127\.0\.0\.1:\d+/);n++) {if(vite.exitCode!==null||n>300)throw new Error(log);await new Promise(r=>setTimeout(r,50));}
  const origin=log.match(/http:\/\/127\.0\.0\.1:\d+/)[0];
  browser=await chromium.launch({executablePath:process.env.PDF_TEST_CHROMIUM||['/usr/bin/chromium','/opt/google/chrome/chrome'].find(existsSync),args:['--no-sandbox']});
  for(const native of [false,true]) {
    const page=await browser.newPage({viewport:{width:480,height:900}});page.setDefaultTimeout(10000);
    const calls=[],errors=[],cancels=[],importCalls=[];let releaseLate,failOnce=false,sessionId='',fingerprint='fp-1',rejectCommit=true;
    page.on('pageerror',e=>errors.push(e.message));
    if(native) await page.addInitScript(()=>{
      const pending=new Map();window.nativeCancels=[];
      window.__TAURI_INTERNALS__={invoke:async(command,args)=>{
        if(command==='cancel_api_request'){window.nativeCancels.push(args.requestId);pending.get(args.requestId)?.abort();return;}
        if(command!=='api')throw new Error(command);
        const abort=new AbortController();if(args.requestId)pending.set(args.requestId,abort);
        try {
          const response=await fetch('/api/v1'+args.path,{method:args.method,signal:abort.signal,headers:{'Content-Type':'application/json',...(args.requestId?{[args.path==='/bib/search/sessions'?'x-discovery-session-id':'x-search-request-id']:args.requestId}:{})},body:args.body==null?undefined:JSON.stringify(args.body)});
          const data=await response.json();if(!response.ok)throw new Error(data.error);return {data};
        } finally {if(args.requestId)pending.delete(args.requestId);}
      }};
    });
    const item=(position,title,edge=true)=>({position,article:{id:'s2:'+('b'.repeat(40)),title,year:2024,identifiers:[{kind:'s2',value:'b'.repeat(40)}]},unresolved:null,citation:edge?{citing_paper_id:'a'.repeat(40),cited_paper_id:'b'.repeat(40)}:null,contexts:edge?['Original citation context']:null,intents:null,is_influential:null});
    await page.route('**/api/v1/bib/search/**',async route=>{
      const path=new URL(route.request().url()).pathname;
      if(path.endsWith('/cancel')) {cancels.push(route.request().postDataJSON());await route.fulfill({status:202,body:''});return;}
      if(path.includes('/search/sessions')) {
        const req=route.request(),input=req.postDataJSON();importCalls.push({path,method:req.method(),input});
        const article={id:'source-paper',title:fingerprint==='fp-1'?'Fresh import metadata':'Refreshed import metadata',identifiers:[{kind:'s2',value:'b'.repeat(40)}]};
        const candidate={id:'candidate',fingerprint,article,records:[{source:'semantic_scholar',article}],conflicts:[],related_versions:[],possible_duplicates:[],first_seen:0,rrf_score:1};
        const session={id:sessionId,sequence:7,phase:'idle',merged_candidates:1,sources:[]};
        let response;
        if(path.endsWith('/sessions')&&req.method()==='POST') {sessionId=req.headers()['x-discovery-session-id'];assert.ok(sessionId);assert.deepEqual(input.native,{source:'semantic_scholar',parameters:{endpoint:'lookup',paper_id:'b'.repeat(40)}});response={session:{...session,id:sessionId}};}
        else if(req.method()==='DELETE') {await route.fulfill({status:204,body:''});return;}
        else if(path.endsWith('/results')) response={candidates:[candidate],obtained_candidates:1,next_offset:null,sequence:7};
        else if(path.endsWith('/events')) response={events:[],sequence:7,reset_required:false};
        else if(path.endsWith('/import/preview')) {assert.equal(input.selections[0].expected_fingerprint,fingerprint);assert.equal(input.library_id,1);assert.equal(input.fetch_fulltext,false);assert.equal(input.articles,undefined);response={preview:{library_id:1,collection_id:null,editable:true,items:[{candidate_id:'candidate',existing:[{id:'manual',title:'User curated title'}],related_existing:[],differences:[],error:null}]}};}
        else if(path.endsWith('/import')) {assert.equal(input.selections[0].expected_fingerprint,fingerprint);if(rejectCommit){rejectCommit=false;fingerprint='fp-2';await route.fulfill({status:409,contentType:'application/json',body:JSON.stringify({error:'Metadata changed'})});return;}response={imported:1,failed:0,items:[{candidate_id:'candidate',status:'existing',article:{id:'manual',title:'User curated title'},error:null}],acquisition:null};}
        else response={session};
        await route.fulfill({contentType:'application/json',body:JSON.stringify(response)});return;
      }
      assert.ok(path.endsWith('/explore'));const input=route.request().postDataJSON();calls.push({input,headers:route.request().headers()});
      if(failOnce){failOnce=false;await route.fulfill({status:429,contentType:'application/json',body:JSON.stringify({error:'Provider rate limit; retry later'})});return;}
      if(input.kind.kind==='citations'&&input.seed==='a'.repeat(40)) await new Promise(resolve=>releaseLate=resolve);
      const beta=input.seed==='c'.repeat(40),similar=input.kind.kind==='similar';
      let items=similar?[item(0,input.kind.pool==='all-cs'?'Computer science recommendation':'Recent recommendation',false)]:input.kind.kind==='citations'?[item(0,'Late Alpha citation')]:input.cursor?[item(25,'Next reference')]:Array.from({length:25},(_,n)=>item(n,n===0?(beta?'Beta reference':'Alpha reference'):`Reference ${n}`));
      if(!similar&&!input.cursor&&input.kind.kind==='references') items[1]={...item(1,'unused'),article:null,unresolved:{title:'Unresolved source entry'},citation:null,contexts:null};
      const result={seed:input.seed,kind:input.kind,source:'semantic_scholar',retrieved_at:'2026-10-28T00:00:00Z',items,unresolved_records:items.filter(i=>!i.article).length,next_cursor:!similar&&input.kind.kind==='references'&&!input.cursor?'next-window':null,upstream_snapshot:false};
      await route.fulfill({contentType:'application/json',body:JSON.stringify(result)}).catch(()=>{});
    });
    await page.goto(origin+'/tests/literature-exploration.html');
    await page.evaluate(async()=>{
      await import('/src/app.css');const {default:Refresh}=await import('/@react-refresh');
      Refresh.injectIntoGlobalHook(window);window.$RefreshReg$=()=>{};window.$RefreshSig$=()=>type=>type;window.__vite_plugin_react_preamble_installed__=true;
      const {mountExploration}=await import('/tests/literature-exploration-mount.tsx');mountExploration(document.getElementById('exploration-root'));
    });
    await page.getByRole('heading',{name:'Explore literature'}).waitFor();assert.equal(calls.length,0);
    await page.getByRole('button',{name:'Load list',exact:true}).click();await page.getByRole('link',{name:'Alpha reference',exact:true}).waitFor();
    assert.ok(calls[0].headers['x-search-request-id']);await page.getByText('Unresolved source entry',{exact:true}).waitFor();
    await page.getByRole('button',{name:'Review import',exact:true}).first().click();
    const review=page.getByRole('dialog',{name:'Review paper import',exact:true});await review.getByText('Fresh import metadata',{exact:true}).waitFor();
    assert.equal(await review.getByRole('button',{name:'Confirm paper import',exact:true}).isDisabled(),true);
    await review.getByRole('button',{name:'Preview paper import',exact:true}).click();await review.getByText('Already in library: User curated title',{exact:true}).waitFor();
    await review.getByRole('button',{name:'Confirm paper import',exact:true}).click();await review.getByRole('alert').filter({hasText:'Metadata changed'}).waitFor();await review.getByText('Refreshed import metadata',{exact:true}).waitFor();
    assert.equal(await review.getByRole('button',{name:'Confirm paper import',exact:true}).isDisabled(),true);
    await review.getByRole('button',{name:'Preview paper import',exact:true}).click();await review.getByRole('button',{name:'Confirm paper import',exact:true}).click();await review.getByRole('status').filter({hasText:'1 imported'}).waitFor();await review.getByRole('button',{name:'Done',exact:true}).click();await review.waitFor({state:'hidden'});
    assert.equal(importCalls.filter(call=>call.path.endsWith('/import')).length,2);
    const viewport=page.getByRole('region',{name:'References',exact:true});
    await viewport.evaluate(el=>el.scrollTop=180);const scroll=await viewport.evaluate(el=>el.scrollTop);assert.ok(scroll>0);
    await page.getByRole('button',{name:'Similar papers',exact:true}).click();await page.getByRole('link',{name:'Recent recommendation'}).waitFor();
    await page.getByLabel('Recommendation pool').selectOption('all-cs');await page.getByRole('link',{name:'Computer science recommendation'}).waitFor();
    await page.getByText('Semantic Scholar · Recommendations, not citation relationships.',{exact:true}).waitFor();
    failOnce=true;await page.getByRole('button',{name:'Refresh list',exact:true}).click();await page.getByRole('alert').filter({hasText:'Provider rate limit'}).waitFor();
    assert.equal(await page.getByText('No records returned by this source.').count(),0);
    await page.getByRole('button',{name:'Refresh list',exact:true}).click();await page.getByRole('alert').waitFor({state:'hidden'});
    await page.locator('[role="region"][aria-label="Similar papers"][aria-busy="false"]').waitFor();
    const before=calls.length;await page.getByRole('button',{name:'References',exact:true}).click();
    await page.getByRole('link',{name:'Alpha reference',exact:true}).waitFor();assert.equal(calls.length,before);
    assert.equal(await viewport.evaluate(el=>el.scrollTop),scroll);
    await page.getByRole('button',{name:'Load more references',exact:true}).click();await page.getByRole('link',{name:'Next reference',exact:true}).waitFor();
    assert.equal(calls.at(-1).input.cursor,'next-window');
    await page.getByRole('button',{name:'Cited by',exact:true}).click();
    for(let n=0;!releaseLate;n++){if(n>100)throw new Error('No pending citation request');await page.waitForTimeout(20);}
    await page.getByRole('button',{name:'Select Beta',exact:true}).click();await page.getByRole('button',{name:'Load list',exact:true}).click();
    await page.getByRole('link',{name:'Beta reference',exact:true}).waitFor();releaseLate();await page.waitForTimeout(100);
    assert.equal(await page.getByText('Late Alpha citation',{exact:true}).count(),0);
    if(native) assert.ok((await page.evaluate(()=>window.nativeCancels)).length>0);else assert.ok(cancels.length>0);
    assert.ok(await page.evaluate(()=>document.documentElement.scrollWidth<=window.innerWidth));
    await page.screenshot({path:`../../target/bib-p4-validation/p5-exploration-${native?'bridge':'browser'}.png`});
    await page.getByRole('button',{name:'Remove identifier',exact:true}).click();await page.getByText('A DOI, PMID, arXiv or Semantic Scholar identifier is needed.',{exact:true}).waitFor();
    assert.deepEqual(errors,[]);
    console.log(JSON.stringify({transport:native?'simulated_desktop_bridge':'browser_fetch',requests:calls.length,import_requests:importCalls.length,passed:true}));await page.close();
  }
} catch(error) {console.error(log);throw error;}
finally {await browser?.close();if(vite.exitCode===null){vite.kill('SIGTERM');await once(vite,'exit');}}
