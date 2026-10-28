// GUI protocol fixtures. API/storage tests independently verify real captures.
import { chromium } from 'playwright';
import { spawn } from 'node:child_process';
import { once } from 'node:events';
import { existsSync,readFileSync } from 'node:fs';
import assert from 'node:assert/strict';
const vite=spawn(process.execPath,['node_modules/vite/bin/vite.js','--host','127.0.0.1','--port','0'],{stdio:['ignore','pipe','pipe']});
let log='',browser;
vite.stdout.on('data',v=>log+=v);vite.stderr.on('data',v=>log+=v);
try {
  for(let n=0;!log.match(/http:\/\/127\.0\.0\.1:\d+/);n++){if(vite.exitCode!==null||n>300)throw new Error(log);await new Promise(r=>setTimeout(r,50));}
  const origin=log.match(/http:\/\/127\.0\.0\.1:\d+/)[0];
  browser=await chromium.launch({executablePath:process.env.PDF_TEST_CHROMIUM||['/usr/bin/chromium','/opt/google/chrome/chrome'].find(existsSync),args:['--no-sandbox']});
  for(const native of [false,true]) {
    const page=await browser.newPage({viewport:{width:480,height:900},acceptDownloads:true});page.setDefaultTimeout(12000);
    const calls=[],cancels=[],errors=[],records=[],actions=new Map();let liveTitle='Study frozen original',failAfterCommit=true,releaseCancel;
    page.on('pageerror',error=>errors.push(error.message));
    await page.addInitScript(()=>{
      window.blobTypes=[];const create=URL.createObjectURL.bind(URL);URL.createObjectURL=blob=>{window.blobTypes.push(blob.type);return create(blob);};
    });
    if(native) await page.addInitScript(()=>{
      const pending=new Map();window.nativeSaved=[];window.nativeCancels=[];
      window.__TAURI_INTERNALS__={invoke:async(command,args)=>{
        if(command==='cancel_api_request'){window.nativeCancels.push(args.requestId);pending.get(args.requestId)?.abort();return;}
        if(command==='plugin:dialog|save')return '/tmp/frozen-export';
        if(command==='save_bib_export'){window.nativeSaved.push(args.request);return;}
        if(command!=='api')throw new Error(command);
        const abort=new AbortController();if(args.requestId)pending.set(args.requestId,abort);
        try {const response=await fetch('/api/v1'+args.path,{method:args.method,signal:abort.signal,headers:{'Content-Type':'application/json',...(args.requestId?{'x-search-request-id':args.requestId}:{})},body:args.body==null?undefined:JSON.stringify(args.body)});const data=await response.json();if(!response.ok)throw new Error(data.error);return {data};}
        finally {if(args.requestId)pending.delete(args.requestId);}
      }};
    });
    const respond=(route,value,status=200)=>route.fulfill({status,contentType:'application/json',body:JSON.stringify(value)}).catch(()=>{});
    await page.route('**/api/v1/bib/**',async route=>{
      const req=route.request(),url=new URL(req.url()),path=url.pathname.replace('/api/v1/bib','');
      if(path==='/search/cancel'){cancels.push(req.postDataJSON());await route.fulfill({status:202,body:''});return;}
      calls.push({method:req.method(),path,body:req.postDataJSON(),headers:req.headers()});
      if(path==='/search/snapshots'&&req.method()==='POST') {
        const input=req.postDataJSON();assert.deepEqual(Object.keys(input).sort(),['deadline_ms','execution','key','max_items','name']);
        if(input.name==='Cancelled capture'){await new Promise(resolve=>releaseCancel=resolve);await respond(route,{error:'cancelled'},499);return;}
        const prior=actions.get(input.key);
        if(prior){assert.deepEqual(prior.input,input);await respond(route,{snapshot:prior.record},201);return;}
        const discovery=input.execution.kind==='discovery';
        const article={id:'saved-a',title:discovery?'Discovered frozen paper':liveTitle,year:2026,abstract_text:'Original stored abstract'};
        const data=discovery?{kind:'discovery',session:{request:{query:'topic'},sources:[{source:'failed',phase:'failed',failure:{kind:'upstream',message:'Saved source failure'}}]},results:{sequence:7,candidates:[{article}]},termination:'end'}:{kind:'library',pages:[{request:input.execution.search,response:{hits:[{article_id:article.id}],articles:[article],index_generation:'frozen-index'}}],termination:'end'};
        const record={summary:{id:`00000000-0000-4000-8000-${String(records.length+1).padStart(12,'0')}`,name:input.name,kind:data.kind,created_at:'2026-10-28T00:00:00Z',checksum:'e'.repeat(64),payload_bytes:1000,coverage:{result_basis:discovery?'obtained_candidates':'complete_matches',complete_obtained_set:true,sources_exhausted:discovery?false:null,captured_items:1}},capture:{started_at:'2026-10-28T00:00:00Z',finished_at:'2026-10-28T00:00:01Z',runtime:{format_version:1,model:null},data}};
        records.unshift(record);actions.set(input.key,{input,record});
        if(failAfterCommit){failAfterCommit=false;await respond(route,{error:'Capture response was lost. Retry saving.'},503);return;}
        await respond(route,{snapshot:record},201);return;
      }
      if(path==='/search/snapshots'&&req.method()==='GET') {const offset=url.searchParams.has('cursor')?1:0;await respond(route,{snapshots:records.slice(offset,offset+1).map(record=>record.summary),next_cursor:records.length>offset+1?'next-list-page':null});return;}
      const match=path.match(/^\/search\/snapshots\/([^/]+)(\/export)?$/);assert.ok(match,`Unexpected hydration or API call: ${path}`);
      const record=records.find(record=>record.summary.id===match[1]);assert.ok(record);
      if(req.method()==='DELETE'){assert.equal(url.searchParams.get('expected_checksum'),record.summary.checksum);records.splice(records.indexOf(record),1);await respond(route,{deleted:true});return;}
      if(match[2]){const format=req.postDataJSON().format;const title=record.capture.data.pages[0].response.articles[0].title;await respond(route,{file_name:`${record.summary.id}.${format==='bibtex'?'bib':format}`,content_type:format==='json'?'application/json; charset=utf-8':'text/plain; charset=utf-8',content:format==='json'?JSON.stringify(record):format==='ris'?`TY  - JOUR\nTI  - ${title}\nER  -\n`:`@article{frozen,title={${title}}}`,articles:1,result_basis:'complete_matches'});return;}
      await respond(route,{snapshot:record});
    });
    await page.goto(origin+'/tests/search-snapshots.html');
    await page.evaluate(async()=>{await import('/src/app.css');const {default:Refresh}=await import('/@react-refresh');Refresh.injectIntoGlobalHook(window);window.$RefreshReg$=()=>{};window.$RefreshSig$=()=>type=>type;window.__vite_plugin_react_preamble_installed__=true;const {mountSnapshots}=await import('/tests/search-snapshots-mount.tsx');mountSnapshots(document.getElementById('snapshots-root'));});
    await page.getByRole('button',{name:'Save result snapshot',exact:true}).filter({visible:true}).click();
    await page.getByLabel('Snapshot name',{exact:true}).fill('Review library');
    await page.getByRole('button',{name:'Save snapshot',exact:true}).click();await page.getByRole('alert').filter({hasText:'response was lost'}).waitFor();
    liveTitle='Changed live title';await page.getByRole('button',{name:'Retry saving snapshot',exact:true}).click();await page.getByRole('status').filter({hasText:'Saved “Review library”'}).waitFor();
    assert.equal(records.length,1);assert.equal(calls.filter(call=>call.method==='POST'&&call.path==='/search/snapshots')[0].body.key,calls.filter(call=>call.method==='POST'&&call.path==='/search/snapshots')[1].body.key);
    assert.ok(calls.find(call=>call.method==='POST'&&call.path==='/search/snapshots').headers['x-search-request-id']);
    await page.getByRole('button',{name:'Done',exact:true}).click();await page.getByRole('button',{name:'snapshots',exact:true}).click();
    await page.getByRole('button',{name:/^Review library/}).click();await page.getByText('Study frozen original',{exact:true}).waitFor();assert.equal(await page.getByText('Changed live title',{exact:true}).count(),0);
    for(const [index,format] of ['bibtex','ris','json'].entries()) {
      await page.getByLabel('Snapshot export format').selectOption(format);
      if(native){await page.getByRole('button',{name:'Export snapshot',exact:true}).click();await page.waitForFunction(count=>window.nativeSaved.length===count,index+1);assert.ok((await page.evaluate(()=>window.nativeSaved.at(-1).contents)).includes('Study frozen original'));}
      else {const ready=page.waitForEvent('download');await page.getByRole('button',{name:'Export snapshot',exact:true}).click();const download=await ready;assert.ok(readFileSync(await download.path(),'utf8').includes('Study frozen original'));}
      await page.locator('button').filter({hasText:'Export snapshot'}).waitFor();
    }
    if(!native) assert.ok((await page.evaluate(()=>window.blobTypes)).includes('application/json; charset=utf-8'));
    await page.getByRole('button',{name:'discovery',exact:true}).click();await page.getByRole('button',{name:'Save result snapshot',exact:true}).filter({visible:true}).click();
    await page.getByLabel('Snapshot name',{exact:true}).fill('Review discovery');await page.getByRole('button',{name:'Save snapshot',exact:true}).click();await page.getByText('Some sources have more results or did not finish successfully.',{exact:true}).waitFor();await page.getByRole('button',{name:'Done',exact:true}).click();
    await page.getByRole('button',{name:'snapshots',exact:true}).click();await page.getByRole('button',{name:/^Review discovery/}).click();await page.getByText('Discovered frozen paper',{exact:true}).waitFor();
    await page.getByRole('button',{name:'Execution details',exact:true}).click();await page.getByText(/Saved source failure/).waitFor();
    await page.getByRole('button',{name:'Load more snapshots',exact:true}).click();await page.getByRole('button',{name:/^Review library/}).click();await page.getByText('Study frozen original',{exact:true}).waitFor();
    await page.getByRole('button',{name:'Delete snapshot',exact:true}).click();assert.equal(records.length,2);await page.getByRole('button',{name:'Confirm snapshot deletion',exact:true}).click();await page.getByRole('button',{name:/^Review library/}).waitFor({state:'hidden'});assert.equal(records.length,1);
    await page.getByRole('button',{name:'library',exact:true}).click();await page.getByRole('button',{name:'Save result snapshot',exact:true}).filter({visible:true}).click();await page.getByLabel('Snapshot name',{exact:true}).fill('Cancelled capture');await page.getByRole('button',{name:'Save snapshot',exact:true}).click();
    for(let n=0;!releaseCancel;n++){if(n>100)throw new Error('No pending capture');await page.waitForTimeout(20);}
    await page.getByRole('button',{name:'Cancel',exact:true}).click();releaseCancel();await page.waitForTimeout(100);assert.equal(records.length,1);
    if(native) assert.ok((await page.evaluate(()=>window.nativeCancels)).length>0);else assert.ok(cancels.length>0);
    await page.getByRole('button',{name:'snapshots',exact:true}).click();await page.getByRole('button',{name:/^Review discovery/}).click();await page.getByText('Discovered frozen paper',{exact:true}).waitFor();
    assert.deepEqual(errors,[]);assert.ok(await page.evaluate(()=>document.documentElement.scrollWidth<=window.innerWidth));await page.screenshot({path:`../../target/bib-p4-validation/p5-snapshots-${native?'bridge':'browser'}.png`});
    console.log(JSON.stringify({transport:native?'simulated_desktop_bridge':'browser_fetch',requests:calls.length,passed:true}));await page.close();
  }
} catch(error){console.error(log);throw error;}
finally {await browser?.close();if(vite.exitCode===null){vite.kill('SIGTERM');await once(vite,'exit');}}
