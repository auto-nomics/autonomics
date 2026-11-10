// The scientific oracle is checked against the real Rust compiler, then adversarial mutations.
import assert from 'node:assert/strict';
import {spawn} from 'node:child_process';
import {existsSync} from 'node:fs';
import {mkdtemp,readFile,rm} from 'node:fs/promises';
import {join} from 'node:path';
import {tmpdir} from 'node:os';
import {fileURLToPath} from 'node:url';
import {checkScientificPlan,checkScientificScene,currentCandidateLeaf} from './helpers/canvas-live-scientific.mjs';
const candidate = (id, revision, parent = null, status = 'pending') => ({id, base_revision: revision, parent_proposal_id: parent, status});
const candidates = [candidate('old-root', 1), candidate('adopted-child', 1, 'old-root', 'accepted'), candidate('new-root', 2), candidate('new-child', 2, 'new-root')];
assert.equal(currentCandidateLeaf(candidates, 2).id, 'new-child');
assert.throws(() => currentCandidateLeaf([...candidates, candidate('ambiguous', 2)], 2));
assert.throws(() => currentCandidateLeaf([candidate('orphan', 2, 'old-root'), candidate('old-root', 1)], 2));
assert.throws(() => currentCandidateLeaf([candidate('cycle-a', 2, 'cycle-b'), candidate('cycle-b', 2, 'cycle-a')], 2));
const spec=JSON.parse(await readFile(new URL('./fixtures/scientific-egfr-mapk.json',import.meta.url),'utf8'));
const binary=process.env.VISUALIZATION_NATIVE_BIN||fileURLToPath(new URL('../../../target/debug/examples/visualization_native',import.meta.url));assert.ok(existsSync(binary),'build native host first');
const directory=await mkdtemp(join(tmpdir(),'scientific-figure-oracle-')),host=spawn(binary,[join(directory,'figures.sqlite')],{stdio:['ignore','pipe','pipe']});let output='';host.stdout.on('data',b=>output+=b);host.stderr.on('data',b=>output+=b);
try{
 const deadline=Date.now()+60000;while(!output.includes('VISUALIZATION_NATIVE_ADDR=')){assert.equal(host.exitCode,null,output);assert.ok(Date.now()<deadline,'host readiness');await new Promise(r=>setTimeout(r,50));}
 const origin=`http://${output.match(/VISUALIZATION_NATIVE_ADDR=([^\s]+)/)[1]}`;
 const call=async(method,path,body)=>{const r=await fetch(origin+'/api/v1/visualization'+path,{method,headers:{'content-type':'application/json'},...(body===undefined?{}:{body:JSON.stringify(body)})});const v=await r.json();assert.ok(r.ok,JSON.stringify(v));return v;};
 await call('POST','/projects',{id:'oracle',title:'Scientific oracle'});const figure=await call('POST','/projects/oracle/figures',{title:spec.title,format:'excalidraw/v2',document:{elements:[],files:{}}});const path=`/projects/oracle/figures/${figure.id}`;
 const plan=await call('PUT',path+'/planning',{expected_revision:1,expected_plan_version:0,document:spec.brief});assert.equal(plan.status,'ready');checkScientificPlan(plan.document,spec);
 const proposal=await call('POST',path+'/planning/sketch',{expected_revision:1,expected_plan_version:plan.version,summary:'Oracle graph'}),document=(await call('GET',path+'/proposals/'+proposal.id)).document;
 const frames=document.elements.filter(e=>e.type==='frame');document.elements.push({id:'scope',type:'text',x:Math.min(...frames.map(f=>f.x)),y:Math.max(...frames.map(f=>f.y+f.height))+32,width:Math.max(...frames.map(f=>f.x+f.width))-Math.min(...frames.map(f=>f.x)),height:20,fontFamily:9,fontSize:16,lineHeight:1.25,opacity:100,frameId:null,text:spec.scope_caption,originalText:spec.scope_caption});
 checkScientificScene(document,plan.document,spec);
 for(const mutate of [p=>p.edges.reverse().forEach(e=>[e.from,e.to]=[e.to,e.from]),p=>p.nodes.pop(),p=>p.edges[0].evidence.push('https://fabricated.invalid/paper'),p=>p.panels[1].title='Unspecified compartment',p=>p.edges[0]=structuredClone(p.edges[1])]){const broken=structuredClone(plan.document);mutate(broken);assert.throws(()=>checkScientificPlan(broken,spec));}
 for(const mutate of [d=>{const edge=d.elements.find(e=>e.type==='arrow');[edge.startBinding,edge.endBinding]=[edge.endBinding,edge.startBinding];},d=>d.elements.find(e=>e.type==='arrow').customData.evidence.push('https://fabricated.invalid/paper'),d=>{const node=d.elements.find(e=>e.customData?.scientificId==='egf');node.frameId=frames[2].id;},d=>d.elements.find(e=>e.id==='scope').opacity=0,d=>d.elements=d.elements.filter(e=>e.id!=='scope'),d=>d.elements.push({id:'unrequested-claim',type:'text',text:'EGFR directly phosphorylates ERK'}),d=>{const edges=d.elements.filter(e=>e.type==='arrow');edges[0].startBinding=structuredClone(edges[1].startBinding);edges[0].endBinding=structuredClone(edges[1].endBinding);edges[0].customData=structuredClone(edges[1].customData); }]){const broken=structuredClone(document);mutate(broken);assert.throws(()=>checkScientificScene(broken,plan.document,spec));}
 const edited=structuredClone(document),target=checkScientificScene(document,plan.document,spec).target.label;Object.assign(edited.elements.find(e=>e.id===target.id),{text:spec.local_edit.new_label,originalText:spec.local_edit.new_label});checkScientificScene(edited,plan.document,spec,{edited:true});assert.throws(()=>checkScientificScene(document,plan.document,spec,{edited:true}));
 console.log('scientific-oracle: real Rust planning/compiler; exact graph/aliases/sources/panels, visible scope and rejection of reversed, invented, duplicated, hidden or unrequested claims passed');
}finally{host.kill();if(host.exitCode===null&&host.signalCode===null)await new Promise(r=>host.once('exit',r));await rm(directory,{recursive:true,force:true});}
