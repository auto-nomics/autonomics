import {runScientificMultiArt} from './canvas-scientific-multi-art.mjs';
import {checkCompactComposition} from './canvas-compact-composition.mjs';
import {egfrScenario} from './canvas-egfr-evidence.mjs';
// Continuous real Agent evaluation; no saved scene is imported between stages.
import assert from 'node:assert/strict';
import {readFile} from 'node:fs/promises';
import {join} from 'node:path';
import {runLiveEvidencePlanning,checkEvidenceExport,sourceRecords} from './canvas-live-evidence.mjs';
import {runLiveIfngArt} from './canvas-live-ifng-art.mjs';

export function workflowReceptor(planning,pattern=/IFNGR|干扰素.*受体/i,exclude=/STAT1/i) {
 const mentions=planning.document.nodes.filter(node=>pattern.test(node.label));
 const dedicated=mentions.filter(node=>!exclude.test(node.label));
 const candidates=dedicated.length?dedicated:mentions;
 const ids=new Set(candidates.map(node=>node.id));
 const earliest=candidates.filter(node=>{
  const seen=new Set([node.id]),queue=[node.id];
  while(queue.length){const current=queue.shift();for(const edge of planning.document.edges.filter(edge=>edge.to===current)){if(edge.from!==node.id&&ids.has(edge.from))return false;if(!seen.has(edge.from)){seen.add(edge.from);queue.push(edge.from);}}}
  return true;
 });
 assert.equal(earliest.length,1,'choose one most-upstream receptor state, distinct from STAT1 docking states');
 return earliest[0];
}

export function workflowScopeNotes(document) {
 const live=document.elements.filter(element=>!element.isDeleted);
 const graph=live.filter(element=>element.customData?.scientificId||element.type==='arrow');
 const ids=new Set(graph.map(element=>element.id));
 const objects=[...graph,...live.filter(element=>element.type==='text'&&ids.has(element.containerId))];
 assert.ok(objects.length,'a scientific graph is required before locating its notes');
 const bottom=Math.max(...objects.map(element=>{
  const {x,y,width,height}=element,angle=element.angle??0;
  assert.ok([x,y,width,height,angle].every(Number.isFinite),'graph geometry must be finite');
  const points=Array.isArray(element.points)?element.points.map(point=>[x+point[0],y+point[1]]):[[x,y],[x+width,y],[x+width,y+height],[x,y+height]];
  assert.ok(points.length&&points.every(point=>point.every(Number.isFinite)),'graph points must be finite');
  const cx=(Math.min(...points.map(point=>point[0]))+Math.max(...points.map(point=>point[0])))/2;
  const cy=(Math.min(...points.map(point=>point[1]))+Math.max(...points.map(point=>point[1])))/2;
  return Math.max(...points.map(([px,py])=>cy+Math.sin(angle)*(px-cx)+Math.cos(angle)*(py-cy)));
 }));
 return live.filter(element=>element.type==='text'&&!element.containerId&&!element.frameId&&element.y>=bottom-1);
}

export async function runLiveEvidenceWorkflow(context) {
 const {page,call,path,persist,directory,turns}=context;
 const caseName=process.env.VISUALIZATION_LIVE_EVIDENCE_CASE??'ifng';assert.ok(['ifng','egfr-mapk'].includes(caseName),'choose an explicit supported scientific case');
 const scenario=caseName==='egfr-mapk'?egfrScenario:undefined;
 const planningResult=await runLiveEvidencePlanning({...context,scenario});
 assert.equal(turns.length,planningResult.planning_turns);
 assert.ok(turns.length>=3&&turns.length<=4,'at most one explicit source-review correction before sketching');
 const sketch=await call('GET',path),planning=await call('GET',`${path}/planning`);
 await persist('workflow-adopted-sketch.json',sketch);
 const compact=process.env.VISUALIZATION_LIVE_LAYOUT_PREFERENCE==='compact';const initialComposition=compact?checkCompactComposition(planning.document,sketch.scene.document):null;
 const receptor=workflowReceptor(planning,scenario?.receptorPattern,scenario?.receptorExclude);
 const doc=sketch.scene.document,target=doc.elements.find(e=>e.customData?.scientificId===receptor.id);
 assert.ok(target,'illustration targets the actual generated component, not a fixture ID');
 const captions=workflowScopeNotes(doc);
 assert.ok(captions.length,'the generated sketch contains visible scope notes below the graph');
 await persist('workflow-stage-handoff.json',{project:sketch.metadata.project_id,figure:sketch.metadata.id,revision:sketch.metadata.revision,target_id:target.id,note_ids:captions.map(e=>e.id),source:'same-session adopted sketch; no fixture import'});
 const illustration=await runLiveIfngArt({...context,targetId:target.id,noteIds:captions.map(e=>e.id),targetName:scenario?.targetName});
 const illustrated=await call('GET',path);assert.equal(illustrated.metadata.revision,sketch.metadata.revision+1);
 const multi=process.env.VISUALIZATION_LIVE_SECOND_MATERIAL==='1'?await runScientificMultiArt({...context,planning}):null;
 const final=await call('GET',path);
 assert.equal(final.metadata.id,sketch.metadata.id);assert.equal(final.metadata.project_id,sketch.metadata.project_id);
 assert.equal(final.metadata.revision,sketch.metadata.revision+(multi?2:1));
 assert.deepEqual(await call('GET',`${path}/planning`),planning,'artwork does not rewrite the reviewed source plan');
 const finalComposition=compact?checkCompactComposition(planning.document,final.scene.document):null;
 const history=JSON.parse(await readFile(join(directory,'session-history.json'),'utf8')),records=sourceRecords(history);
 const download=page.waitForEvent('download');await page.getByRole('button',{name:'导出素材来源',exact:true}).click();
 const manifest=JSON.parse(await readFile(await(await download).path(),'utf8'));
 checkEvidenceExport(manifest,final.scene.document,records);
 if(multi){const images=final.scene.document.elements.filter(e=>e.type==='image'&&!e.isDeleted);assert.equal(images.length,2);for(const image of images){const item=manifest.assets.find(a=>a.element_id===image.id);assert.ok(item&&item.hash_matches_recorded);assert.deepEqual(item.authors,image.customData.asset.creators);assert.deepEqual(item.licenses,image.customData.asset.licenses);assert.ok(item.source_urls.includes(image.customData.asset.source_url));}}
 await persist('workflow-final-source-manifest.json',manifest);
 return {planning:planningResult,illustration,multi_materialization:multi,composition:{requested:process.env.VISUALIZATION_LIVE_LAYOUT_PREFERENCE??'auto',initial:initialComposition,final:finalComposition},scope:multi?'continuous same-project same-Agent source-backed IFNG figure with two distinct catalogue illustrations and isolated label follow-up; sources and editable export verified, artwork does not establish structural or universal scientific correctness':scenario?.scope??'continuous same-project same-Agent human IFNG source clarification, confirmed sketch, generic receptor illustration and optional local follow-up; source discovery and source-backed export verified, scientific correctness still requires review',fixture_imported:false,continuous_turns:turns.length};
}
