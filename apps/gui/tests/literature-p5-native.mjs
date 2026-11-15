import '/src/app.css';
import { mountBibliography } from './bibliography-menu-mount.tsx';
import { invoke } from '@tauri-apps/api/core';
const evidence=JSON.parse(sessionStorage.getItem('p5-evidence')??'[]');
const visible=node=>node?.getClientRects().length>0;
const wait=async(test,label)=>{const end=Date.now()+30000;while(Date.now()<end){const value=await test();if(value)return value;await new Promise(r=>setTimeout(r,50));}throw new Error(`Timed out: ${label}. Alerts: ${[...document.querySelectorAll('[role=alert]')].map(n=>n.textContent)}`);};
const button=(text,scope=document)=>[...scope.querySelectorAll('button')].find(n=>visible(n)&&(n.textContent.trim()===text||n.getAttribute('aria-label')===text));
const click=async(text,scope=document)=>(await wait(()=>{const n=button(text,scope);return n&&!n.disabled?n:null;},text)).click();
const set=(node,value)=>{Object.getOwnPropertyDescriptor(node instanceof HTMLSelectElement?HTMLSelectElement.prototype:HTMLInputElement.prototype,'value').set.call(node,value);node.dispatchEvent(new Event(node instanceof HTMLSelectElement?'change':'input',{bubbles:true}));};
const api=async(method,path,body=null)=>{const r=await invoke('api',{method,path,body,requestId:null});if(r.status>=400)throw new Error(JSON.stringify(r));return r.data;};
let finishing=false;
const finish=async result=>{if(finishing)return;finishing=true;await invoke('smoke_done',{result:{...result,evidence,runtime:'Tauri WebKitGTK',mock_bridge:false}});};
window.addEventListener('error',e=>void finish({ok:false,error:e.message}));
window.addEventListener('unhandledrejection',e=>void finish({ok:false,error:String(e.reason)}));
try {
  if(!sessionStorage.getItem('p5-stage')) {
    const created=await api('POST','/bib/collections',{name:'Native destination'});
    sessionStorage.setItem('p5-target',created.collection.id);
  }
  mountBibliography(document.body);
  await wait(()=>document.querySelector('[data-article-id="native-curated"]'),'initial library');
  if(sessionStorage.getItem('p5-stage')==='reload') {
    evidence.push('native WebView reload restores library');
    await finish({ok:true});
  } else {
    document.querySelector('[data-article-id="native-curated"] [data-bib-row-primary]').click();
    const explorer=await wait(()=>document.querySelector('[aria-label="Literature exploration"]'),'native exploration');
    await click('Load list',explorer);
    await wait(()=>explorer.textContent.includes('Native unresolved reference'),'unresolved reference retained');
    if(explorer.querySelectorAll('[aria-label="References"] li').length!==2)throw new Error('Raw reference window was truncated');
    await click('Cited by',explorer);
    await wait(()=>explorer.querySelector('[aria-label="Cited by"]')?.textContent.includes('Cites the selected paper'),'citation direction');
    await click('Similar papers',explorer);
    await wait(()=>explorer.querySelector('[aria-label="Similar papers"]')?.textContent.includes('Native fresh graph paper'),'recent recommendations');
    set(explorer.querySelector('select[aria-label="Recommendation pool"]'),'all-cs');
    await wait(async()=>(await invoke('smoke_info')).graph_calls.some(path=>path.includes('from=all-cs')),'explicit recommendation pool reaches SDK');
    await click('References',explorer);await click('Review import',explorer);await click('Preview paper import');
    await wait(()=>document.querySelector('dialog[open]')?.textContent.includes('New library record'),'graph identity preview');
    await click('Confirm paper import');await wait(()=>document.querySelector('dialog[open]')?.textContent.includes('0 failed'),'graph import');await click('Done');
    const graphCalls=(await invoke('smoke_info')).graph_calls;
    if(!graphCalls.some(path=>path.startsWith('/paper/'+ 'b'.repeat(40)+'?')))throw new Error('Graph import did not fetch fresh singleton through SDK');
    evidence.push('native references retain unresolved entries; citation direction; both recommendation pools; fresh SDK graph import');
    sessionStorage.setItem('p5-stage','reload');sessionStorage.setItem('p5-evidence',JSON.stringify(evidence));location.reload();
  }
} catch(error) { await finish({ok:false,error:String(error)}); }
