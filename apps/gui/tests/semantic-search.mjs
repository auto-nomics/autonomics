import { chromium } from 'playwright';
import assert from 'node:assert/strict';
import { spawn } from 'node:child_process';
import { once } from 'node:events';
import { existsSync } from 'node:fs';
import { createHash } from 'node:crypto';

const origin='http://127.0.0.1:1432';
const vite=spawn(process.execPath,['node_modules/vite/bin/vite.js','--host','127.0.0.1','--port','1432','--strictPort'],{stdio:['ignore','pipe','pipe']});
let output='',browser,activePage;
vite.stdout.on('data',data=>output+=data);vite.stderr.on('data',data=>output+=data);
const digest='a'.repeat(64),topic='免疫细胞表达调控 AND NOT title:literal';
const source='First line.\n真实的语义证据来自第二个附件。\nLast line.\n';
const snippet='真实的语义证据来自第二个附件。',hash=createHash('sha256').update(source).digest('hex');
const articles=[{id:'a',title:'Alpha immune research',year:2026},{id:'b',title:'Beta outside scope',year:2010},{id:'c',title:'Gamma related research',year:2026}].map(article=>({...article,authors:[],identifiers:[],abstract_text:'An actual abstract.'}));
const defaults=()=>({enabled:false,origin:'http://127.0.0.1:11434',allow_remote:false,model:'qwen3-embedding:0.6b',model_digest:'',profile:{dimensions:1024,query_instruction:'Given a scientific claim, retrieve scientific papers that provide relevant evidence.',chunk_chars:null,overlap_chars:0,include_annotations:false,include_fulltext:false},batch_size:16,timeout_seconds:120});
const hard={op:'and',clauses:[{op:'range',field:'year',gte:'2020',lte:'2026'},{op:'not',clause:{op:'term',field:'tag',value:'excluded'}}]};
try {
  for(let n=0;!output.includes(origin);n++){if(vite.exitCode!==null||n>150)throw new Error(output);await new Promise(resolve=>setTimeout(resolve,50));}
  browser=await chromium.launch({executablePath:process.env.PDF_TEST_CHROMIUM||['/usr/bin/chromium','/opt/google/chrome/chrome'].find(existsSync),args:['--no-sandbox']});
  for(const native of [false,true]) {
    const page=await browser.newPage({viewport:{width:1440,height:1000},acceptDownloads:true});activePage=page;page.setDefaultTimeout(12000);
    const calls=[],errors=[];let rules=[],config=defaults(),phase='disabled',configGeneration='config-0',readyAt=0,maintenance=null,progress=null;
    const evidence={doc_id:'SECOND:attachment',anchor_key:'SECOND',kind:'attachment',matched_field:'fulltext',content_hash:'canonical-content-hash',start_byte:0,end_byte:100,snippet};
    const status=()=>{
      if(phase==='indexing'&&Date.now()>=readyAt){phase='ready';if(maintenance?.state==='running'){maintenance.state='complete';progress={...progress,stage:'complete',processed_articles:3,processed_chunks:3};maintenance.progress=progress;}}
      return {config,phase,configuration_generation:configGeneration,generation:phase==='disabled'?null:'vectors-1',signature:'model-signature',indexed_revision:1,indexed_chunks:phase==='disabled'?0:3,resources:phase==='disabled'?null:{vector_bytes:3072,disk_bytes:4096},last_error:phase==='failed'?'Fixture model is unavailable':null,maintenance,progress};
    };
    page.on('pageerror',error=>errors.push(error.message));
    if(native) await page.addInitScript(()=>{
      const pending=new Map();window.nativeCalls=[];
      window.__TAURI_INTERNALS__={invoke:async(command,args)=>{
        window.nativeCalls.push({command,args});
        if(command==='cancel_api_request'){pending.get(args.requestId)?.abort();return;}
        if(command==='plugin:dialog|save')return '/tmp/topic-export.bib';
        if(command==='save_bib_export')return;
        if(command==='bib_fulltext_raw') {
          const url='/api/v1/bib/articles/'+encodeURIComponent(args.articleId)+'/attachments/'+encodeURIComponent(args.attachmentKey)+'/raw?expected_hash='+encodeURIComponent(args.expectedHash);
          const response=await fetch(url);const bytes=new Uint8Array(await response.arrayBuffer());return {content_type:response.headers.get('content-type'),data_base64:btoa(Array.from(bytes,byte=>String.fromCharCode(byte)).join(''))};
        }
        if(command!=='api')throw new Error('Unexpected command '+command);
        const abort=new AbortController();if(args.requestId)pending.set(args.requestId,abort);
        try {
          const response=await fetch('/api/v1'+args.path,{method:args.method,signal:abort.signal,headers:{'Content-Type':'application/json',...(args.requestId?{'x-search-request-id':args.requestId}:{})},body:args.body==null?undefined:JSON.stringify(args.body)});
          const text=await response.text(),data=text?JSON.parse(text):null;if(!response.ok)throw new Error(data?.error||String(response.status));return {data};
        }finally{pending.delete(args.requestId);}
      }};
    });
    await page.route(new RegExp('^'+origin.replaceAll('.','\\.')+'/?(?:\\?.*)?$'),route=>route.fulfill({contentType:'text/html',body:'<!doctype html><html lang="en"><body></body></html>'}));
    await page.route('**/api/v1/**',async route=>{
      const request=route.request(),url=new URL(request.url()),path=url.pathname.replace('/api/v1',''),body=request.postDataJSON();calls.push({path,method:request.method(),body,headers:request.headers()});
      const reply=(json,status=200)=>route.fulfill({json,status}).catch(()=>{});
      if(path==='/bib/collections')return reply({collections:[]});
      if(path==='/bib/collections/changes')return reply({});
      if(path==='/bib/health')return reply({article_count:3});
      if(path==='/bib/search/status')return reply({engine:'tantivy',source_revision:1,indexed_revision:1,indexed_documents:3,pending_articles:0,building:false,processed_documents:0,durable:true});
      if(path==='/bib/articles')return reply({articles,total:3});
      if(path==='/bib/search/cancel')return reply({},202);
      if(path==='/bib/search/semantic/maintenance') {
        if(maintenance?.request_id===body.request_id)return reply({maintenance,status:status()},202);
        assert.equal(body.expected_configuration_generation,configGeneration);assert.match(body.request_id,/^[0-9a-f-]{36}$/);
        configGeneration+='-job';phase=body.action==='cancel'?'paused':'indexing';readyAt=Date.now()+800;
        progress={stage:'embedding',target_revision:1,target_articles:3,processed_articles:1,processed_chunks:1,embedded_chunks:body.action==='repair'?1:0,cached_chunks:body.action==='repair'?0:1};
        maintenance={request_id:body.request_id,action:body.action,configuration_generation:configGeneration,state:body.action==='cancel'?'cancelled':'running',progress,last_error:null,store_generation:body.action==='repair'?body.request_id:maintenance?.store_generation??null};
        return reply({maintenance,status:status()},202);
      }
      if(path==='/bib/search/semantic/models')return reply({models:[{name:'qwen3-embedding:0.6b',digest}]});
      if(path==='/bib/search/semantic') {
        if(request.method()==='PUT'){config=structuredClone(body);maintenance=null;progress=null;configGeneration+='-next';phase=config.enabled?'indexing':'disabled';readyAt=Date.now()+200;return reply({configuration_generation:configGeneration,status:status()},202);}
        return reply(status());
      }
      if(path==='/bib/saved-searches') {
        if(request.method()==='POST'){const rule={id:'topic-rule',name:body.name,schema_version:1,version:1,request:body.request};rules.push(rule);return reply({saved_search:rule},201);}
        return reply({saved_searches:rules});
      }
      if(path==='/bib/search/v2') {
        if(body.mode!=='hybrid')return reply({articles:[articles[0]],total:1,hits:[],index_generation:'lexical-1'});
        if(!config.enabled||status().phase!=='ready')return reply({error:'Topic model is not ready'},503);
        assert.ok(body.semantic_query.trim());
        if(body.semantic_query==='slow topic')await new Promise(resolve=>setTimeout(resolve,650));
        if(body.cursor==='expired')return reply({error:'Fixture candidate cursor expired'},400);
        if(body.semantic_query==='failure topic')return reply({error:'Fixture model query failed'},503);
        const rows=body.cursor?[articles[2]]:[articles[0]];
        return reply({articles:rows,total:2,next_cursor:body.cursor?null:body.semantic_query==='expired paging'?'expired':'next-page',index_generation:'hybrid-1',hits:rows.map(article=>({article_id:article.id,title:article.title,score:.1,snippet,highlight_ranges:[],matched_field:'fulltext',anchor_key:'SECOND'})),retrieval:{algorithm:'weighted_rrf_v1',total_kind:'candidate_pool',facets_basis:'hard_query_disjunctive_filters',eligible_articles:2,candidate_depth:100,semantic_generation:'vectors-1',configuration_generation:configGeneration,indexed_chunks:3,hits:Object.fromEntries(rows.map(article=>[article.id,{lexical_rank:article.id==='c'?2:null,semantic_rank:article.id==='c'?2:1,semantic_evidence:evidence}]))}});
      }
      if(path==='/bib/search/export')return reply({format:'bib',count:2,result_basis:'candidate_pool',export:'@article{a,title={Alpha immune research}}'});
      if(path.endsWith('/search-evidence'))return reply({index_generation:'hybrid-1',total:1,next_cursor:null,evidence:[{article_id:'a',item_key:'PARENT',attachment_key:'SECOND',attachment_hash:hash,matched_field:'fulltext',snippet,highlight_ranges:[],matched_query:snippet,page:null,content_revision:'hybrid-1',location_status:'text_search',semantic_evidence:evidence}]});
      if(path==='/bib/articles/a')return reply({article:articles[0],annotations:[],fulltext:{file_format:'txt',attachment_key:'FIRST'},attachments:[{key:'FIRST',link_mode:0,content_type:'text/plain',title:'Preferred file',path:'first.txt'},{key:'SECOND',link_mode:0,content_type:'text/plain',title:'Matching file',path:'second.txt',storage_hash:hash}]});
      if(path==='/bib/articles/a/attachments/SECOND/raw'){assert.equal(url.searchParams.get('expected_hash'),hash);return route.fulfill({contentType:'text/plain; charset=utf-8',body:source});}
      if(path.endsWith('/annotations'))return reply({annotations:[]});
      throw new Error(`Unhandled fixture ${request.method()} ${path}`);
    });
    const mount=()=>page.evaluate(async()=>{
      await import('/src/app.css');const {default:Refresh}=await import('/@react-refresh');Refresh.injectIntoGlobalHook(window);window.$RefreshReg$=()=>{};window.$RefreshSig$=()=>type=>type;window.__vite_plugin_react_preamble_installed__=true;
      const {mountBibliography}=await import('/tests/bibliography-menu-mount.tsx');mountBibliography(document.body);
    });
    const input=()=>page.getByRole('textbox',{name:'Search bibliography'});
    const latest=()=>calls.findLast(call=>call.path==='/bib/search/v2'&&call.body.mode==='hybrid')?.body;
    await page.goto(origin);await mount();await input().fill('year:2026');await input().press('Enter');
    await page.getByLabel('Search mode').selectOption('hybrid');
    assert.equal(await page.getByLabel('Required search conditions').inputValue(),'year:2026');
    assert.equal(await input().inputValue(),'');assert.equal(latest(),undefined);
    await page.getByRole('button',{name:'Topic settings',exact:true}).click();
    await page.getByLabel('Installed embedding model').waitFor();
    await page.getByText('Advanced model and content options',{exact:true}).click();
    assert.equal(await page.getByLabel('Semantic context tokens').inputValue(),'');
    await page.getByLabel('Semantic context tokens').fill('8192');
    await page.getByLabel('Semantic token batch').fill('8192');
    await page.getByText('Index resource limits',{exact:true}).click();
    assert.equal(await page.getByLabel('Semantic vector budget MiB').inputValue(),'2048');
    await page.getByLabel('Semantic maximum chunks').fill('1000');
    await page.getByLabel('Semantic maximum input KiB').fill('64');
    await page.getByLabel('Semantic vector budget MiB').fill('64');
    await page.getByLabel('Semantic disk budget MiB').fill('256');
    const dialogBox=await page.getByRole('dialog').boundingBox();
    for(const control of [page.getByRole('heading',{name:'Topic search settings',exact:true}),page.getByRole('button',{name:'Save topic settings',exact:true})]) {
      const box=await control.boundingBox();assert.ok(box&&dialogBox&&box.y>=dialogBox.y&&box.y+box.height<=dialogBox.y+dialogBox.height,'settings title and save must remain inside the visible dialog while editing its last field');
    }
    await page.screenshot({path:`/tmp/bib-semantic-resource-settings-${native?'bridge':'web'}.png`});
    await page.getByRole('checkbox',{name:'Enable indexing of this selected library text',exact:true}).check();
    await page.getByRole('button',{name:'Save topic settings',exact:true}).click();
    await page.getByLabel('Topic search status').getByText('Topic index ready',{exact:true}).waitFor();
    assert.equal(config.model_digest,digest);assert.equal(config.allow_remote,false);
    assert.deepEqual(config.inference,{num_ctx:8192,num_batch:8192});
    assert.deepEqual(config.limits,{max_chunks:1000,max_input_bytes:65536,max_vector_bytes:64*1024**2,max_disk_bytes:256*1024**2});
    assert.match(await page.getByLabel('Topic cache usage').textContent(),/3.0 KiB vector payload.*4.0 KiB/);
    await page.getByRole('button',{name:'Topic settings',exact:true}).click();
    await page.getByText('Advanced model and content options',{exact:true}).click();
    assert.equal(await page.getByLabel('Semantic context tokens').inputValue(),'8192');
    await page.getByText('Index resource limits',{exact:true}).click();
    assert.equal(await page.getByLabel('Semantic maximum chunks').inputValue(),'1000');
    assert.equal(await page.getByLabel('Semantic disk budget MiB').inputValue(),'256');
    assert.equal(await page.getByLabel('Semantic token batch').inputValue(),'8192');
    await page.getByLabel('Semantic context tokens').fill('');
    await page.getByLabel('Semantic token batch').fill('');
    await page.getByRole('button',{name:'Save topic settings',exact:true}).click();
    await page.getByRole('dialog').waitFor({state:'detached'});
    assert.deepEqual(config.inference,{num_ctx:null,num_batch:null});
    const before=calls.filter(call=>call.path==='/bib/search/v2'&&call.body.mode==='hybrid').length;
    await input().fill(topic);await page.waitForTimeout(300);
    assert.equal(calls.filter(call=>call.path==='/bib/search/v2'&&call.body.mode==='hybrid').length,before,'typing a topic must not send embeddings');
    await input().dispatchEvent('compositionstart');await input().press('Enter');
    assert.equal(calls.filter(call=>call.path==='/bib/search/v2'&&call.body.mode==='hybrid').length,before,'IME Enter must not submit a topic');
    await input().dispatchEvent('compositionend');
    await input().press('Enter');await page.getByText('Semantic similarity',{exact:true}).waitFor();
    await page.getByText('Keyword + semantic',{exact:true}).waitFor();
    assert.equal(latest().semantic_query,topic);assert.equal(latest().query,'year:2026');
    assert.equal(await page.locator('mark').count(),0);
    await page.getByText('2 candidate articles',{exact:true}).waitFor();
    assert.equal((await page.getByLabel('Articles in library').textContent()).trim(),'3','library count must not display the candidate pool size');
    await page.getByRole('button',{name:'Rebuild topic index',exact:true}).click();
    await page.getByLabel('Topic index progress').waitFor();
    await page.getByLabel('Topic maintenance state').filter({hasText:'rebuild · complete'}).waitFor();
    await page.getByRole('button',{name:'Repair topic index',exact:true}).click();
    await page.getByLabel('Topic index progress').waitFor();
    await page.getByRole('button',{name:'Pause indexing',exact:true}).click();
    await page.getByText('Topic indexing paused',{exact:true}).waitFor();
    await page.getByRole('button',{name:'Resume indexing',exact:true}).click();
    await page.getByLabel('Topic maintenance state').filter({hasText:'resume · complete'}).waitFor();
    await page.getByText('2 candidate articles',{exact:true}).waitFor();
    assert.deepEqual(calls.filter(call=>call.path==='/bib/search/semantic/maintenance').map(call=>call.body.action),['rebuild','repair','cancel','resume']);
    await page.getByRole('button',{name:'Save current search',exact:true}).click();
    await page.getByLabel('Search name').fill('Immune topic');await page.getByRole('button',{name:'Save rule',exact:true}).click();
    await page.getByRole('button',{name:'⌕ Immune topic',exact:true}).waitFor();
    assert.equal(rules[0].request.semantic_query,topic);assert.equal(rules[0].request.query,'year:2026');assert.equal(rules[0].request.mode,'hybrid');
    const rule={id:'structured-topic',name:'Structured topic',schema_version:1,version:1,request:{mode:'hybrid',semantic_query:topic,expression:hard,scope:{include_descendants:true},filters:{has_fulltext:true}}};rules.push(rule);
    await page.evaluate(()=>window.dispatchEvent(new Event('focus')));
    await Promise.all([page.waitForRequest(request=>request.url().endsWith('/search/v2')&&request.postDataJSON()?.expression?.op==='and'),page.getByRole('button',{name:'⌕ Structured topic',exact:true}).click()]);
    await page.waitForFunction(()=>JSON.parse(new URL(location.href).searchParams.get('bib_search')).request.expression?.op==='and');
    assert.deepEqual(latest().expression,hard);
    await page.reload();await mount();await page.getByText('Semantic similarity',{exact:true}).waitFor();
    assert.equal(await page.getByLabel('Search mode').inputValue(),'hybrid');assert.equal(await input().inputValue(),topic);assert.deepEqual(latest().expression,hard);assert.equal(latest().query,'');
    await page.getByRole('button',{name:/Alpha immune research/}).click();
    await page.getByRole('button',{name:'Open evidence',exact:true}).waitFor();
    await page.getByText('Selected by semantic similarity; no literal keyword match is implied.',{exact:true}).waitFor();
    const viewport=page.getByLabel('Literature results viewport'),scroll=await viewport.evaluate(node=>node.scrollTop);
    await page.getByRole('button',{name:'Open evidence',exact:true}).click();
    const reader=page.locator('.zotero-pdf-reader[data-reader-kind=txt]');await reader.waitFor();await reader.getByRole('button',{name:/Match 1/}).waitFor();
    await reader.locator('[data-action=close]').click();await reader.waitFor({state:'detached'});assert.equal(await viewport.evaluate(node=>node.scrollTop),scroll);
    assert.equal(calls.some(call=>call.path.endsWith('/FIRST/raw')),false);
    if(native){await page.getByRole('button',{name:'Export BibTeX',exact:true}).click();await page.waitForFunction(()=>window.nativeCalls.some(call=>call.command==='save_bib_export'));}
    else {const download=page.waitForEvent('download');await page.getByRole('button',{name:'Export BibTeX',exact:true}).click();await download;}
    assert.deepEqual(calls.findLast(call=>call.path==='/bib/search/export').body.search.expression,hard);
    await input().fill('expired paging');await input().press('Enter');
    await page.getByRole('alert').filter({hasText:'Fixture candidate cursor expired'}).first().waitFor();
    await page.waitForTimeout(400);
    assert.equal(calls.filter(call=>call.path==='/bib/search/v2'&&call.body.cursor==='expired').length,1,'an expired continuation must stop automatic paging');
    await input().fill('slow topic');await Promise.all([page.waitForRequest(request=>request.url().endsWith('/search/v2')&&request.postDataJSON()?.semantic_query==='slow topic'),input().press('Enter')]);
    await page.getByRole('button',{name:'Cancel search',exact:true}).click();
    await page.getByText('Search cancelled. Previous results are retained.',{exact:true}).waitFor();
    await input().fill(topic);await input().press('Enter');await page.waitForTimeout(800);assert.equal(latest().semantic_query,topic);
    if(native)assert.ok(await page.evaluate(()=>window.nativeCalls.some(call=>call.command==='cancel_api_request')));
    else assert.ok(calls.some(call=>call.path==='/bib/search/cancel'));
    await input().fill('failure topic');await input().press('Enter');await page.getByRole('alert').filter({hasText:'Fixture model query failed'}).first().waitFor();
    assert.equal(await page.getByRole('button',{name:/Alpha immune research/}).count(),1);
    await page.getByRole('button',{name:'Use keyword search',exact:true}).click();await page.waitForFunction(()=>document.querySelector('[aria-label="Search mode"]')?.value==='lexical');
    assert.equal(await input().inputValue(),'(year:2020..2026 AND NOT (tag:excluded))');
    assert.ok(await page.evaluate(topic=>!(JSON.parse(localStorage.getItem('autonomics.bib.searchHistory.v1')||'[]')).includes(topic),topic),'topics must not enter keyword query history');
    assert.ok(await page.evaluate(topic=>(JSON.parse(localStorage.getItem('autonomics.bib.topicHistory.v1')||'[]')).includes(topic),topic));
    await page.getByLabel('Search mode').selectOption('hybrid');
    await page.getByRole('button',{name:'Topic settings',exact:true}).click();
    await page.getByLabel('Semantic model server').fill('https://embedding.example');
    assert.equal(await page.getByRole('button',{name:'Load installed models',exact:true}).isDisabled(),true);
    await page.getByRole('button',{name:'Save topic settings',exact:true}).click();await page.getByRole('dialog').getByRole('alert').filter({hasText:'explicitly'}).waitFor();
    await page.getByRole('checkbox',{name:'Allow sending the selected library text to this remote server',exact:true}).check();
    await page.getByRole('button',{name:'Save topic settings',exact:true}).click();
    await page.getByRole('dialog',{name:'Topic search settings'}).waitFor({state:'detached'});
    assert.equal(config.allow_remote,true);assert.equal(config.origin,'https://embedding.example');
    await page.getByLabel('Topic search status').getByText(/remote model server: https:\/\/embedding.example/).waitFor();
    await page.getByLabel('Topic search status').getByText('Topic index ready',{exact:true}).waitFor();
    await input().fill(topic);await input().press('Enter');await page.getByText('2 candidate articles',{exact:true}).waitFor();
    await page.setViewportSize({width:760,height:900});
    if(await page.getByRole('button',{name:'Close panel',exact:true}).count())await page.getByRole('button',{name:'Close panel',exact:true}).click();
    await page.waitForFunction(()=>[...document.querySelectorAll('aside')].every(node=>{const rect=node.getBoundingClientRect();return rect.right<=1||rect.left>=innerWidth-1;}));
    assert.ok(await page.evaluate(()=>document.documentElement.scrollWidth<=innerWidth+1),'narrow layout should not overflow horizontally');
    await page.screenshot({path:`/tmp/bib-semantic-${native?'bridge':'web'}.png`});
    assert.deepEqual(errors,[]);console.log(`${native?'Simulated Tauri bridge':'Browser'}: topic/settings/consent/saved AST/URL/recall/reader/export/cancel/failure passed`);
    await page.close();activePage=null;
  }
} catch(error) {if(activePage)await activePage.screenshot({path:'/tmp/bib-semantic-failure.png'}).catch(()=>{});throw error;}
finally {await browser?.close();vite.kill('SIGTERM');await once(vite,'exit');}
