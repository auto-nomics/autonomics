import { chromium } from 'playwright';
import { spawn } from 'node:child_process';
import { once } from 'node:events';
import { existsSync } from 'node:fs';
import assert from 'node:assert/strict';
const vite=spawn(process.execPath,['node_modules/vite/bin/vite.js','--host','127.0.0.1','--port','0'],{stdio:['ignore','pipe','pipe']});
let log='',browser;vite.stdout.on('data',v=>log+=v);vite.stderr.on('data',v=>log+=v);
try {
  for(let n=0;!log.match(/http:\/\/127\.0\.0\.1:\d+/);n++){if(vite.exitCode!==null||n>600)throw new Error(log);await new Promise(r=>setTimeout(r,50));}
  const origin=log.match(/http:\/\/127\.0\.0\.1:\d+/)[0];
  browser=await chromium.launch({executablePath:process.env.PDF_TEST_CHROMIUM||['/usr/bin/chromium','/opt/google/chrome/chrome'].find(existsSync),args:['--no-sandbox']});
  for(const native of [false,true]) {
    const page=await browser.newPage({viewport:{width:480,height:900}});page.setDefaultTimeout(20000);
    const calls=[],errors=[],checks=[],previews=[],commits=[];let rule=null,firstAckLost=true,freshValid=false,sessionId='',cancelled=0,collectionsAvailable=true;
    const now=Date.now(),runs=[];const limits={max_rules:128,max_seen:100000,max_seen_bytes:268435456,max_inbox:10000,max_inbox_bytes:134217728,max_page_bytes:2097152,max_checkpoints:100000,max_runs:100000,max_run_bytes:4194304,max_history_bytes:268435456};
    const article=(id,title)=>({id,title,identifiers:[{kind:'openalex',value:`https://openalex.org/${id}`},{kind:'doi',value:'10.1000/fixture'}]});
    const record=(a)=>({source:'openalex',article:a,source_rank:1,retrieved_at:new Date(now).toISOString(),executed_parameters:{},query_translation:null});
    let items=[{id:'inbox-valid',rule_id:'rule-1',rule_version:1,first_matched_ms:now,records:[record(article('W123456','Stored notification title'))],related_groups:[],read:false},{id:'inbox-unresolved',rule_id:'rule-1',rule_version:1,first_matched_ms:now,records:[record({id:'unresolved',title:'Unresolved source title',identifiers:[]})],related_groups:['review-version'],read:false}];
    page.on('pageerror',e=>errors.push(e.message));
    if(native) await page.addInitScript(()=>{window.__TAURI_INTERNALS__={invoke:async(command,args)=>{if(command==='cancel_api_request')return;if(command!=='api')throw new Error(command);const response=await fetch('/api/v1'+args.path,{method:args.method,headers:{'content-type':'application/json',...(args.requestId?{'x-search-request-id':args.requestId,'x-discovery-session-id':args.requestId}:{})},body:args.body==null?undefined:JSON.stringify(args.body)});const data=response.status===204?null:await response.json();if(!response.ok)throw new Error(data.error);return {data};}};});
    const respond=(route,value,status=200)=>route.fulfill({status,contentType:'application/json',body:status===204?'':JSON.stringify(value)}).catch(()=>{});
    await page.route('**/api/v1/bib/**',async route=>{
      const req=route.request(),url=new URL(req.url()),p=url.pathname.replace('/api/v1/bib',''),method=req.method(),body=req.postDataJSON();calls.push({p,method,body});
      if(p==='/search/cancel'){await respond(route,{},202);return;}
      if(p==='/collections'){await respond(route,{collections:collectionsAvailable?[{id:'team',name:'Team collection',library_id:1},{id:'other',name:'Other collection',library_id:1}]:[]});return;}
      if(p.startsWith('/search/sessions')) {
        const fresh=article(freshValid?'W123456':'W999999','Fresh verified source metadata');
        const candidate={id:'candidate',fingerprint:'fresh-fingerprint',article:fresh,records:[record(fresh)],conflicts:[],related_versions:[],possible_duplicates:[],first_seen:0,rrf_score:1};
        const session={id:sessionId,sequence:7,phase:'idle',merged_candidates:1,sources:[]};
        if(p==='/search/sessions'&&method==='POST'){sessionId=req.headers()['x-discovery-session-id'];assert.ok(sessionId);assert.deepEqual(body.native,{source:'openalex',parameters:{filter:'openalex:W123456'}});await respond(route,{session:{...session,id:sessionId}});return;}
        if(method==='DELETE'){await respond(route,{},204);return;}
        if(p.endsWith('/results')){await respond(route,{candidates:[candidate],obtained_candidates:1,next_offset:null,sequence:7});return;}
        if(p.endsWith('/events')){await respond(route,{events:[],sequence:7,reset_required:false});return;}
        if(p.endsWith('/import/preview')){previews.push(body);assert.equal(body.collection_id,'team');assert.equal(body.articles,undefined);assert.deepEqual(body.selections,[{candidate_id:'candidate',expected_fingerprint:'fresh-fingerprint',record_index:0}]);await respond(route,{preview:{library_id:1,collection_id:'team',editable:true,items:[{candidate_id:'candidate',existing:[{id:'curated',title:'User curated title'}],related_existing:[],differences:[],error:null}]}});return;}
        if(p.endsWith('/import')){commits.push(body);assert.equal(body.collection_id,'team');await respond(route,{imported:1,failed:0,items:[{candidate_id:'candidate',status:'existing',article:{id:'curated',title:'User curated title'},error:null}],acquisition:null});return;}
        await respond(route,{session});return;
      }
      if(p==='/subscriptions/limits'){if(method==='PUT'){Object.assign(limits,body);if(rule)rule.resource_wait=null;}await respond(route,{limits});return;}
      if(p==='/subscriptions/status'){await respond(route,{scheduler:{running:true,active:runs.some(r=>r.state==='running')?1:0,last_error:null}});return;}
      if(p==='/subscriptions'&&method==='POST'){assert.equal(body.enabled,false);assert.equal(body.request.filters.year_from,2020);assert.deepEqual(body.request.filters.publication_types,['article']);rule={id:'rule-1',version:1,spec:body,next_due_ms:now,last_success_ms:null,sources:[{source:'openalex',watermark_ms:null,complete:false,cursor:'next',last_checked_ms:now,last_error:null,retry_at_ms:null,window:null,plan:{parameters:{search:'immune'}},scan:{strategy:'bounded_replay',done:false,target_ms:null}}]};await respond(route,{rule},201);return;}
      if(p==='/subscriptions'){await respond(route,{rules:rule?[rule]:[]});return;}
      if(p==='/subscriptions/inbox'){await respond(route,{items:url.searchParams.has('before')?[]:items,next:url.searchParams.has('before')?null:5});return;}
      if(p.startsWith('/subscriptions/inbox/')){const id=p.split('/').at(-1);if(method==='DELETE')items=items.filter(i=>i.id!==id);else items.find(i=>i.id===id).read=body.read;await respond(route,{},204);return;}
      if(p==='/subscriptions/rule-1/check') {
        checks.push(body.run_id);let run=runs.find(r=>r.id===body.run_id);
        if(!run){run={id:body.run_id,rule_id:'rule-1',rule_version:rule.version,started_ms:now+runs.length*1000,finished_ms:runs.length?null:now+1,state:runs.length?'running':'partial',reason:'Page budget exhausted',pages_started:1,pages_committed:1,records:1,new_notifications:1,spec:rule.spec,sources:[{source:'openalex',strategy:'bounded_replay',done:false,error:null,windows:[]}]};runs.unshift(run);}
        if(firstAckLost){firstAckLost=false;await respond(route,{error:'Check response was lost; retry acknowledgement'},503);return;}await respond(route,{run},202);return;
      }
      if(p==='/subscriptions/rule-1/runs'){await respond(route,{runs:url.searchParams.has('before')?[]:runs,next:url.searchParams.has('before')?null:10});return;}
      if(p.startsWith('/subscriptions/runs/')){const id=p.split('/')[3],run=runs.find(r=>r.id===id);if(p.endsWith('/cancel')){cancelled++;run.state='cancelled';run.finished_ms=now+100;await respond(route,{run});}else if(method==='DELETE'){runs.splice(runs.indexOf(run),1);await respond(route,{},204);}else await respond(route,{run});return;}
      if(p==='/subscriptions/rule-1/enabled'){assert.equal(body.version,rule.version);rule.spec.enabled=body.enabled;rule.version++;await respond(route,{rule});return;}
      if(p==='/subscriptions/rule-1'&&method==='PUT'){assert.equal(body.version,rule.version);rule.spec=body.spec;rule.version++;await respond(route,{rule});return;}
      if(p==='/subscriptions/rule-1'&&method==='DELETE'){assert.equal(Number(url.searchParams.get('version')),rule.version);rule=null;items=[];runs.length=0;await respond(route,{},204);return;}
      throw new Error(`Unexpected ${method} ${p}`);
    });
    await page.goto(origin+'/tests/subscriptions.html');
    await page.evaluate(async()=>{await import('/src/app.css');const {default:Refresh}=await import('/@react-refresh');Refresh.injectIntoGlobalHook(window);window.$RefreshReg$=()=>{};window.$RefreshSig$=()=>type=>type;window.__vite_plugin_react_preamble_installed__=true;const {mountSubscriptions}=await import('/tests/subscriptions-mount.tsx');mountSubscriptions(document.getElementById('subscriptions-root'));});
    await page.getByRole('button',{name:'Subscribe to this search',exact:true}).click();await page.getByLabel('Subscription name',{exact:true}).fill('Immune watch');await page.getByRole('button',{name:'Save subscription',exact:true}).click();await page.getByText('Subscription saved. Review it in Subscriptions.').waitFor();assert.equal(checks.length,0);
    await page.getByRole('button',{name:'subscriptions',exact:true}).click();await page.getByRole('button',{name:'Immune watch',exact:true}).click();
    await page.getByRole('button',{name:'Check now',exact:true}).click();await page.getByRole('alert').filter({hasText:'response was lost'}).waitFor();await page.getByRole('button',{name:'Retry check acknowledgement',exact:true}).click();await page.getByText('This check is incomplete; it does not mean there were no new matches.').waitFor();assert.equal(checks[0],checks[1]);assert.equal(runs.length,1);
    await page.getByRole('button',{name:'Load more checks',exact:true}).click();await page.getByRole('button',{name:'Load more checks',exact:true}).waitFor({state:'hidden'});
    await page.getByRole('button',{name:'Resume',exact:true}).click();await page.getByRole('button',{name:'Pause',exact:true}).click();await page.getByRole('button',{name:'Resume',exact:true}).waitFor();
    await page.getByRole('button',{name:'Edit rule',exact:true}).click();await page.getByLabel('Subscription name',{exact:true}).fill('Edited immune watch');await page.getByRole('button',{name:'Save subscription',exact:true}).click();await page.getByRole('button',{name:'Edited immune watch',exact:true}).waitFor();assert.equal(rule.spec.request.filters.year_from,2020);
    rule.resource_wait={since_ms:now,reason:'inbox quota'};await page.getByRole('button',{name:'Refresh subscriptions',exact:true}).click();await page.getByText(/Waiting for storage: inbox quota/).waitFor();assert.ok(await page.getByRole('button',{name:'Check now',exact:true}).isDisabled());
    await page.getByRole('button',{name:'Storage limits',exact:true}).click();await page.getByLabel('Inbox notifications',{exact:true}).fill('10001');await page.getByRole('button',{name:'Save storage limits',exact:true}).click();await page.getByRole('dialog',{name:'Subscription storage limits'}).waitFor({state:'hidden'});await page.getByRole('button',{name:'Check now',exact:true}).waitFor({state:'visible'});assert.ok(await page.getByRole('button',{name:'Check now',exact:true}).isEnabled());
    await page.getByRole('button',{name:'Load more notifications',exact:true}).click();await page.getByRole('button',{name:'Load more notifications',exact:true}).waitFor({state:'hidden'});
    await page.getByText('No reliable supported identifier is available for a fresh lookup.',{exact:true}).waitFor();await page.getByLabel('Subscription import destination').selectOption('team');
    await page.getByRole('button',{name:'Review notification import',exact:true}).click();let review=page.getByRole('dialog',{name:'Review paper import',exact:true});await review.getByText(/No verified paper was obtained/).waitFor();assert.ok(await review.getByRole('button',{name:'Preview paper import',exact:true}).isDisabled());await review.getByRole('button',{name:'Cancel',exact:true}).click();assert.equal(previews.length,0);
    freshValid=true;await page.getByRole('button',{name:'Review notification import',exact:true}).click();review=page.getByRole('dialog',{name:'Review paper import',exact:true});await review.getByText('Fresh verified source metadata',{exact:true}).waitFor();
    await page.evaluate(()=>{const select=document.querySelector('select[aria-label="Subscription import destination"]');const setter=Object.getOwnPropertyDescriptor(HTMLSelectElement.prototype,'value').set;setter.call(select,'other');select.dispatchEvent(new Event('change',{bubbles:true}));});
    await review.getByText('Import target: Team collection',{exact:true}).waitFor();await review.getByRole('button',{name:'Preview paper import',exact:true}).click();await review.getByText('Already in library: User curated title',{exact:true}).waitFor();await review.getByRole('button',{name:'Confirm paper import',exact:true}).click();await review.getByText('1 imported · 0 failed',{exact:true}).waitFor();await review.getByRole('button',{name:'Done',exact:true}).click();assert.equal(commits.length,1);
    await page.getByRole('button',{name:'Mark read',exact:true}).first().click();await page.getByRole('button',{name:'Mark unread',exact:true}).waitFor();
    await page.getByRole('button',{name:'Check now',exact:true}).click();await page.getByRole('button',{name:'Cancel check',exact:true}).click();await page.getByRole('button',{name:'Cancel check',exact:true}).waitFor({state:'hidden'});assert.equal(cancelled,1);
    collectionsAvailable=false;await page.getByRole('button',{name:'discover',exact:true}).click();await page.getByRole('button',{name:'subscriptions',exact:true}).click();await page.getByRole('button',{name:'Edited immune watch',exact:true}).waitFor();assert.equal(cancelled,1);
    await page.getByRole('option',{name:'Unavailable collection',exact:true}).waitFor({state:'attached'});const lookupsBefore=calls.filter(c=>c.p==='/search/sessions'&&c.method==='POST').length;await page.getByRole('button',{name:'Review notification import',exact:true}).click();await page.getByRole('alert').filter({hasText:'selected collection is unavailable'}).waitFor();assert.equal(calls.filter(c=>c.p==='/search/sessions'&&c.method==='POST').length,lookupsBefore);
    await page.getByRole('region',{name:'Literature subscriptions'}).evaluate(element=>element.scrollTop=0);await page.screenshot({path:`../../target/bib-p4-validation/p5-subscriptions-${native?'bridge':'browser'}-populated.png`});
    await page.getByRole('button',{name:'Delete notification',exact:true}).last().click();await page.getByRole('button',{name:'Confirm deletion',exact:true}).click();await page.getByText('Unresolved source title',{exact:true}).waitFor({state:'hidden'});
    await page.getByRole('button',{name:'Delete rule',exact:true}).click();assert.ok(rule);await page.getByRole('button',{name:'Confirm deletion',exact:true}).click();await page.getByText('No subscriptions yet.',{exact:true}).waitFor();assert.equal(rule,null);await page.getByText('Stored notification title',{exact:true}).waitFor({state:'hidden'});
    assert.deepEqual(errors,[]);assert.ok(await page.evaluate(()=>document.documentElement.scrollWidth<=window.innerWidth));await page.screenshot({path:`../../target/bib-p4-validation/p5-subscriptions-${native?'bridge':'browser'}.png`});console.log(JSON.stringify({transport:native?'simulated_desktop_bridge':'browser_fetch',requests:calls.length,idempotent_check:true,identifier_mismatch_blocked:true,frozen_import_target:true,passed:true}));await page.close();
  }
} catch(error){console.error(log);throw error;}
finally{await browser?.close();if(vite.exitCode===null){vite.kill('SIGTERM');await once(vite,'exit');}}
