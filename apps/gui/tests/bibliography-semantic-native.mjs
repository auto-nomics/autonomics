import '/src/app.css';
import { mountBibliography } from './bibliography-menu-mount.tsx';
import { invoke } from '@tauri-apps/api/core';

const topic='gene editing native evidence in the second file',hard='year:2020..2026';
const evidence=JSON.parse(sessionStorage.getItem('semanticNativeEvidence')||'[]');
const wait=async(test,label)=>{const end=Date.now()+30000;while(Date.now()<end){const value=test();if(value)return value;await new Promise(resolve=>setTimeout(resolve,30));}throw new Error('Timed out: '+label);};
const button=text=>[...document.querySelectorAll('button')].find(node=>node.textContent.trim()===text||node.getAttribute('aria-label')===text);
const setInput=(input,value)=>{Object.getOwnPropertyDescriptor(HTMLInputElement.prototype,'value').set.call(input,value);input.dispatchEvent(new Event('input',{bubbles:true}));};
const select=(input,value)=>{input.value=value;input.dispatchEvent(new Event('change',{bubbles:true}));};
const finish=result=>invoke('smoke_done',{result});
const api=(method,path,body=null)=>invoke('api',{method,path,body,requestId:null}).then(response=>response.data);
const waitApi=async(test,label)=>{const end=Date.now()+30000;while(Date.now()<end){const value=await test();if(value)return value;await new Promise(resolve=>setTimeout(resolve,50));}throw new Error('Timed out: '+label);};
window.addEventListener('error',event=>void finish({ok:false,error:event.message,evidence}));
window.addEventListener('unhandledrejection',event=>void finish({ok:false,error:String(event.reason),evidence}));
try {
  mountBibliography(document.body);
  const input=await wait(()=>document.querySelector('input[aria-label="Search bibliography"]'),'search input');
  if(sessionStorage.getItem('semanticNativeStage')==='restore') {
    const reloaded=(await api('GET','/bib/search/semantic')).config;
    if(reloaded.inference?.num_ctx!==8192||reloaded.inference?.num_batch!==8192)throw new Error('Reload lost explicit model runtime options');
    if(reloaded.limits?.max_chunks!==100||reloaded.limits?.max_disk_bytes!==32*1024**2)throw new Error('Reload lost semantic resource limits');
    await wait(()=>document.querySelector('[data-article-id="native-a"]'),'restored results');
    if(document.querySelector('[aria-label="Search mode"]').value!=='hybrid'||input.value!==topic||document.querySelector('[aria-label="Required search conditions"]').value!==hard)throw new Error('Reload lost topic, mode or hard conditions');
    const saved=await api('GET','/bib/saved-searches');
    const rule=saved.saved_searches.find(rule=>rule.name==='Native topic review');
    if(rule?.request.mode!=='hybrid'||rule.request.semantic_query!==topic||rule.request.query!==hard)throw new Error('Persisted rule lost topic or hard conditions');
    evidence.push('real WebView reload restores topic and hard conditions','backend persisted hybrid saved rule');
    const maintenance=[];
    for(const action of ['rebuild','repair','cancel','resume']) {
      const prior=await api('GET','/bib/search/semantic');
      const label={rebuild:'Rebuild topic index',repair:'Repair topic index',cancel:'Pause indexing',resume:'Resume indexing'}[action];
      (await wait(()=>{const node=button(label);return node&&!node.disabled?node:null;},label)).click();
      const finished=await waitApi(async()=>{const status=await api('GET','/bib/search/semantic');return status.configuration_generation!==prior.configuration_generation&&status.maintenance?.action===action&&(action==='cancel'?status.phase==='paused':status.maintenance.state==='complete')?status:null;},action+' completion');
      await wait(()=>document.querySelector('[aria-label="Topic maintenance state"]')?.textContent.includes(action+' · '+(action==='cancel'?'cancelled':'complete')),'visible '+action+' completion');
      maintenance.push({action,generation:finished.generation,store_generation:finished.store_generation,progress:finished.maintenance.progress});
      if(action==='cancel') {
        (await wait(()=>button('Topic settings'),'paused resource settings')).click();
        (await wait(()=>[...document.querySelectorAll('dialog summary')].find(node=>node.textContent==='Index resource limits'),'resource section')).click();
        setInput(document.querySelector('[aria-label="Semantic maximum chunks"]'),'101');
        (await wait(()=>{const node=button('Save topic settings');return node&&!node.disabled?node:null;},'save paused resource budget')).click();
        const paused=await waitApi(async()=>{const status=await api('GET','/bib/search/semantic');return status.configuration_generation!==finished.configuration_generation&&status.config.limits.max_chunks===101?status:null;},'paused budget persisted');
        if(paused.phase!=='paused'||paused.maintenance?.request_id!==finished.maintenance.request_id||paused.maintenance?.store_generation!==finished.maintenance.store_generation)throw new Error('Budget save lost or resumed explicit pause');
        evidence.push('budget-only GUI save retains the paused maintenance identity until explicit resume');
      }
    }
    evidence.push('GUI rebuild, isolated repair, persistent pause and resume through real IPC/API/model');
    (await wait(()=>button('Topic settings'),'settings after reload')).click();
    const enabled=await wait(()=>document.querySelector('dialog input[type="checkbox"]'),'enable checkbox');
    if(!enabled.checked)throw new Error('Index was not enabled after reload');enabled.click();
    (await wait(()=>button('Save topic settings'),'disable indexing')).click();
    await wait(()=>document.querySelector('[aria-label="Topic search status"]')?.textContent.includes('Topic indexing is off'),'disabled index status');
    (await wait(()=>button('Use keyword search'),'keyword fallback')).click();
    await wait(()=>document.querySelector('[aria-label="Search mode"]').value==='lexical','explicit fallback');
    if(input.value!==hard)throw new Error('Keyword fallback changed hard conditions');
    evidence.push('GUI persists disable and explicitly returns to keywords');
    sessionStorage.removeItem('semanticNativeStage');
    await finish({ok:true,runtime:'Tauri WebKitGTK',model:'actual installed Qwen3-Embedding 0.6B',relevance_evaluation:false,evidence,maintenance,hybrid_probe:JSON.parse(sessionStorage.getItem('semanticNativeProbe'))});
  } else {
    await wait(()=>document.querySelector('[data-article-id="native-a"]'),'initial library');
    setInput(input,hard);input.dispatchEvent(new KeyboardEvent('keydown',{key:'Enter',bubbles:true}));
    await wait(()=>new URL(location.href).searchParams.get('bib_search')?.includes('2020'),'hard query');
    select(document.querySelector('[aria-label="Search mode"]'),'hybrid');
    await wait(()=>document.querySelector('[aria-label="Required search conditions"]')?.value===hard,'mode retains hard query');
    (await wait(()=>{const node=button('Topic settings');return node&&!node.disabled?node:null;},'topic settings')).click();
    const installed=await wait(()=>document.querySelector('[aria-label="Installed embedding model"]'),'actual model discovery via native IPC');
    select(installed,'qwen3-embedding:0.6b');
    const enable=document.querySelector('dialog input[type="checkbox"]');if(!enable.checked)enable.click();
    [...document.querySelectorAll('dialog summary')].find(node=>node.textContent.includes('Advanced model')).click();
    setInput(document.querySelector('[aria-label="Semantic context tokens"]'),'8192');
    setInput(document.querySelector('[aria-label="Semantic token batch"]'),'8192');
    [...document.querySelectorAll('dialog summary')].find(node=>node.textContent==='Index resource limits').click();
    setInput(document.querySelector('[aria-label="Semantic maximum chunks"]'),'100');
    setInput(document.querySelector('[aria-label="Semantic vector budget MiB"]'),'8');
    setInput(document.querySelector('[aria-label="Semantic disk budget MiB"]'),'32');
    document.querySelector('[aria-label="Semantic disk budget MiB"]').focus();
    const dialogBounds=document.querySelector('dialog').getBoundingClientRect();
    for(const node of [document.querySelector('dialog h2'),button('Save topic settings')]) {
      const bounds=node.getBoundingClientRect();
      if(bounds.top<dialogBounds.top||bounds.bottom>dialogBounds.bottom)throw new Error('Settings title or save scrolled out of the native dialog');
    }
    const fulltext=[...document.querySelectorAll('dialog label')].find(node=>node.textContent.trim()==='Include extracted attachment text')?.querySelector('input');
    if(!fulltext)throw new Error('Full-text scope missing');if(!fulltext.checked)fulltext.click();
    (await wait(()=>{const node=button('Save topic settings');return node&&!node.disabled?node:null;},'save installed model')).click();
    await wait(()=>document.querySelector('[aria-label="Topic search status"]')?.textContent.includes('Topic index ready'),'actual model background indexing');
    const enabledStatus=await api('GET','/bib/search/semantic');
    if(enabledStatus.config.inference?.num_ctx!==8192||enabledStatus.config.inference?.num_batch!==8192)throw new Error('Native settings did not persist explicit model runtime options');
    if(enabledStatus.config.limits?.max_chunks!==100||enabledStatus.config.limits?.max_vector_bytes!==8*1024**2||!enabledStatus.resources?.vector_bytes)throw new Error('Native resource limits or actual cache usage missing');
    evidence.push('resource limits and nonzero vector cache usage through real IPC, with limits retained on page reload');
    evidence.push('explicit context/token-batch options persist across real IPC and page reload');
    evidence.push('actual installed model metadata via HTTP and native IPC','GUI enables full-text scope and fixed digest indexing');
    setInput(input,topic);(await wait(()=>button('Search topic'),'topic submit')).click();
    await wait(()=>document.querySelector('[data-article-id="native-a"]')?.textContent.includes('semantic'),'hybrid recall explanation');
    const request=JSON.parse(new URL(location.href).searchParams.get('bib_search')).request;
    const page=await api('POST','/bib/search/v2',{...request,include_articles:true});
    const detail=page.retrieval?.hits['native-a'];
    if(page.total!==1||page.retrieval.total_kind!=='candidate_pool'||!detail?.semantic_evidence)throw new Error('Native request lost hybrid response metadata');
    if(detail.semantic_evidence.kind!=='attachment'||!detail.semantic_evidence.snippet.includes('second file'))throw new Error('Actual semantic candidate did not identify the matching attachment');
    sessionStorage.setItem('semanticNativeProbe',JSON.stringify({generation:page.index_generation,total:page.total,recall:detail,config:(await api('GET','/bib/search/semantic')).config}));
    evidence.push('real model query with required year filter','candidate count and dual recall explanation');
    document.querySelector('[data-article-id="native-a"] [data-bib-row-primary]').click();
    const sourceRow=await wait(()=>[...document.querySelectorAll('[aria-label="Search evidence"] p.font-medium')].find(node=>node.textContent.startsWith('Related evidence'))?.parentElement,'semantic source evidence');
    const open=[...sourceRow.querySelectorAll('button')].find(node=>node.textContent.trim()==='Open evidence');
    if(!open)throw new Error('Semantic evidence has no attachment action');open.click();
    const reader=await wait(()=>document.querySelector('.zotero-pdf-reader[data-reader-kind="txt"]'),'semantic source reader');
    await wait(()=>[...reader.querySelectorAll('button')].some(node=>node.textContent.includes('Match 1')),'real semantic source text location');
    if(!reader.textContent.includes('second file')||reader.textContent.includes('Unrelated preferred file.'))throw new Error('Opened wrong semantic attachment');
    reader.querySelector('[data-action="close"]').click();await wait(()=>!document.querySelector('.zotero-pdf-reader'),'return to results');
    if(input.value!==topic)throw new Error('Reader return lost topic');
    evidence.push('semantic source opens non-preferred attachment through binary IPC','reader locates actual source text and returns without changing topic');
    (await wait(()=>button('Save current search'),'save topic action')).click();
    setInput(await wait(()=>document.querySelector('[aria-label="Search name"]'),'rule name'),'Native topic review');
    (await wait(()=>{const node=button('Save rule');return node&&!node.disabled?node:null;},'save rule')).click();
    await wait(()=>button('⌕ Native topic review'),'saved topic rule');
    sessionStorage.setItem('semanticNativeEvidence',JSON.stringify(evidence));sessionStorage.setItem('semanticNativeStage','restore');
    location.reload();
  }
} catch(error) {await finish({ok:false,error:String(error),evidence});}
