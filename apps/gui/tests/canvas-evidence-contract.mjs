import { workflowScopeNotes } from './helpers/canvas-live-evidence-workflow.mjs';
import {checkPortableSceneExport} from './helpers/canvas-live-source-review.mjs';
import assert from 'node:assert/strict';
import {checkEvidencePlan,checkEvidenceClarification,checkEvidenceExport,checkSketchPlanLifecycle,evidenceUrls,unreadEvidenceUrls,sourceRecords} from './helpers/canvas-live-evidence.mjs';
import {relationSources} from '../src/features/canvas/canvas-source-manifest.ts';
const first={version:1,document:{questions:[{id:'species',question:'Which species?',answer:''},{id:'subtype',question:'Which interferon subtype?',answer:''}]}};
const signedZeroScene={files:{},elements:[{id:'frame',type:'frame',x:-0,y:0,width:300,height:400,angle:-0,points:[[0,-0],[10,20]],customData:{source:'retained'}}]};
const serializedScene=JSON.parse(JSON.stringify(signedZeroScene));
checkPortableSceneExport(serializedScene,signedZeroScene);
for(const mutate of [scene=>scene.elements[0].x=1e-12,scene=>scene.elements[0].points[1][0]=11,scene=>scene.elements[0].customData.source='changed']){
 const invalid=structuredClone(serializedScene);mutate(invalid);assert.throws(()=>checkPortableSceneExport(invalid,signedZeroScene));
}
const emptyBindingScene={elements:[{id:'image',type:'image',boundElements:null,customData:{note:null}}],files:{}};
const emptyBindingExport=structuredClone(emptyBindingScene);emptyBindingExport.elements[0].boundElements=[];
checkPortableSceneExport(emptyBindingExport,emptyBindingScene);
checkPortableSceneExport(emptyBindingScene,emptyBindingExport);
for(const mutate of [scene=>scene.elements[0].boundElements=[{id:'new-owner',type:'text'}],scene=>scene.elements[0].customData.note=[]]){
 const invalid=structuredClone(emptyBindingExport);mutate(invalid);assert.throws(()=>checkPortableSceneExport(invalid,emptyBindingScene));
}
const boundScene=structuredClone(emptyBindingExport);boundScene.elements[0].boundElements=[{id:'owned-label',type:'text'}];
assert.throws(()=>checkPortableSceneExport(emptyBindingExport,boundScene));
const changedOwner=structuredClone(boundScene);changedOwner.elements[0].boundElements[0].id='other-label';assert.throws(()=>checkPortableSceneExport(changedOwner,boundScene));
const phosphorylation='https://reactome.org/content/detail/R-HSA-873922',transport='https://reactome.org/content/detail/R-HSA-873917',overview='https://reactome.org/content/detail/R-HSA-877300';
const records=[['R-HSA-873922',phosphorylation],['R-HSA-873917',transport],['R-HSA-877300',overview]].map(([id,url])=>({record:{stId:id},source_url:url}));
const navigationHistory=[{content:[
 {type:'tool_use',id:'navigation',name:'figure_evidence',input:{action:'events',id:'R-HSA-877300'}},
 {type:'tool_result',tool_use_id:'navigation',content:{coverage:'pathway_contained_event_navigation_only',results:[{id:'R-HSA-873922'}]}},
 {type:'tool_use',id:'read',name:'figure_evidence',input:{action:'record',id:'R-HSA-873917'}},
 {type:'tool_result',tool_use_id:'read',content:records[1]},
]}];
assert.deepEqual(sourceRecords(navigationHistory),[records[1]],'contained event navigation does not grant read evidence for listed reactions');
const plan={status:'ready',version:2,document:{message:'Human IFN-gamma-to-nuclear STAT1 abstraction',nodes:[{id:'receptor',label:'IFNG:IFNGR1/2:JAK1/2 complex'},{id:'phospho',label:'p-STAT1 (Y701)'},{id:'dimer',label:'p-STAT1 homodimer'},{id:'nuclear',label:'Nuclear p-STAT1 homodimer'}],edges:[{from:'receptor',to:'phospho',label:'STAT1 phosphorylation (summary)',evidence:[phosphorylation]},{from:'phospho',to:'dimer',label:'homodimer formation',evidence:[phosphorylation]},{from:'dimer',to:'nuclear',label:'nuclear translocation',evidence:[transport]}],assumptions:['JAK2 responsibility is probable, not uniquely assigned; detailed cofactors omitted.'],questions:[{id:'species',question:'Which species?',answer:'Homo sapiens'},{id:'subtype',question:'Which interferon subtype?',answer:'IFN-gamma'}]}};
const pending={...first,status:'needs_clarification'};
const unfiltered={name:'figure_evidence',input:{action:'search',query:'interferon'},is_error:false};
const comparison={name:'figure_evidence',input:{action:'search',query:'STAT1',species:'Mus musculus'},is_error:false};
checkEvidenceClarification(pending,[unfiltered,comparison]);
assert.throws(()=>checkEvidenceClarification(pending,[comparison,unfiltered]),/initial discovery must not assume organism/);
assert.throws(()=>checkEvidenceClarification(pending,[{...unfiltered,is_error:true}]),/actual tool activity/);
const assumed=structuredClone(pending);assumed.document.questions[0].answer='Human';assert.throws(()=>checkEvidenceClarification(assumed,[unfiltered]));
checkEvidencePlan(plan,first,records);
const residueLabel=structuredClone(plan);residueLabel.document.nodes[1].label='STAT1 pTyr701 (membrane)';checkEvidencePlan(residueLabel,first,records);
const disconnected=structuredClone(plan);disconnected.document.nodes.push({id:'unmodified',label:'Unmodified STAT1'});disconnected.document.nodes[1].label='STAT1 pTyr701 bound to p-IFNGR1';disconnected.document.edges[0].from='unmodified';assert.throws(()=>checkEvidencePlan(disconnected,first,records),/declared signal has a directed path/);
const descriptive=structuredClone(plan);for(const edge of descriptive.document.edges)edge.evidence=edge.evidence.map(url=>`Reactome record — ${url}（curated source）`);checkEvidencePlan(descriptive,first,records);
const optional=structuredClone(plan);optional.document.questions.push({id:'language',question:'Language preference?',answer:'',blocking:false});const withOptional=structuredClone(first);withOptional.document.questions.push({id:'language',question:'Language preference?',answer:'',blocking:false});checkEvidencePlan(optional,withOptional,records);

const contextual=structuredClone(plan),contextualFirst=structuredClone(first);const preference={id:'cell_context',question:'是否需要标出应答细胞类型？IFN-γ 的细胞背景可能影响解读。',answer:'',blocking:false};contextual.document.questions.push(preference);contextualFirst.document.questions.push(preference);checkEvidencePlan(contextual,contextualFirst,records);
const style=structuredClone(plan),styleFirst=structuredClone(first);const optionalStyle={id:'style',question:'配色/风格偏好（如按亚型区分色系、是否需要深浅两版）？',answer:'',blocking:false};style.document.questions.push(optionalStyle);styleFirst.document.questions.push(optionalStyle);checkEvidencePlan(style,styleFirst,records);
const downgraded=structuredClone(plan);downgraded.document.questions[1].blocking=false;downgraded.document.questions[1].answer='';assert.throws(()=>checkEvidencePlan(downgraded,first,records));

const exportDocument={elements:plan.document.edges.map((edge,i)=>({id:`edge_${i}`,type:'arrow',customData:{evidence:[...edge.evidence,'Abstraction: intermediate states combined; this note is not a source URL.']}}))};
const exported={relation_sources:relationSources(exportDocument)};checkEvidenceExport(exported,exportDocument,records);
const alteredExport=structuredClone(exported);alteredExport.relation_sources.relations[0].references[0].text=transport;assert.throws(()=>checkEvidenceExport(alteredExport,exportDocument,records),/preserves each supplied/);
const inventedDocument=structuredClone(exportDocument);inventedDocument.elements[0].customData.evidence=['https://example.org/invented'];assert.throws(()=>checkEvidenceExport({relation_sources:relationSources(inventedDocument)},inventedDocument,records),/URLs were actually read/);

for(const mutate of [p=>{p.document.questions.shift();},p=>{p.document.questions[0].answer='';},p=>{p.document.nodes[1].label='STAT2';},p=>{p.document.assumptions=['JAK2 is certainly the only kinase'];},p=>{p.document.edges[0].evidence=[overview];},p=>{p.document.edges[2].evidence=[phosphorylation];p.document.edges[0].evidence=[transport];},p=>{p.document.edges[0].evidence=['https://example.org/invented'];}]){const changed=structuredClone(plan);mutate(changed);assert.throws(()=>checkEvidencePlan(changed,first,records));}
console.log('evidence-contract: retained answers, type-II branch, JAK uncertainty, actually read sources and transition-specific citations; rejects overview-only and swapped evidence');

// A real evaluated plan used “nuclear GAF” after explicitly defining its identity.
const gaf=structuredClone(plan),release='https://reactome.org/content/detail/R-HSA-873927';
const gafRecords=[...records,{source_url:release,record:{stId:'R-HSA-873927',displayName:'Release of STAT1 dimer',output:[{displayName:'GAF (p-STAT1 homodimer)'}]}}];
gaf.document.nodes[2].label='p-STAT1 homodimer (GAF)';gaf.document.nodes[2].evidence=[release];
gaf.document.nodes[3].label='细胞核（核内 GAF）';gaf.document.nodes[3].evidence=[transport];
checkEvidencePlan(gaf,first,gafRecords);
assert.throws(()=>checkEvidencePlan(gaf,first,records),'an unread alias-definition source is insufficient');
const undefinedAlias=structuredClone(gaf);undefinedAlias.document.nodes[2].label='p-STAT1 homodimer';assert.throws(()=>checkEvidencePlan(undefinedAlias,first,gafRecords),'GAF must be explicitly tied to STAT1');
const unlinkedAlias=structuredClone(gaf);unlinkedAlias.document.nodes[2].evidence=[];assert.throws(()=>checkEvidencePlan(unlinkedAlias,first,gafRecords),'the alias definition keeps its read source');
const unrelatedAliasRecord=structuredClone(gafRecords);unrelatedAliasRecord.at(-1).record={stId:'R-HSA-873927',displayName:'Unrelated record'};assert.throws(()=>checkEvidencePlan(gaf,first,unrelatedAliasRecord),'a source ID alone does not establish the alias');
const wrongAliasSource=structuredClone(gaf);wrongAliasSource.document.nodes[3].evidence=[phosphorylation];assert.throws(()=>checkEvidencePlan(wrongAliasSource,first,gafRecords),/nuclear GAF alias/);
const disconnectedAlias=structuredClone(gaf);disconnectedAlias.document.edges[1].from='nuclear';assert.throws(()=>checkEvidencePlan(disconnectedAlias,first,gafRecords),/declared signal has a directed path/);
console.log('evidence-contract: source-defined GAF nuclear alias accepted; missing definitions, unread or unrelated records, wrong nuclear source and disconnected endpoint rejected');


const reviewed={...plan,based_on_revision:7,proposal_id:null,updated_at:'2026-11-11T08:00:00Z'};
const proposed={...reviewed,status:'proposed',proposal_id:'sketch-root',updated_at:'2026-11-11T08:01:00Z'};
const sketchRoot={id:'sketch-root',base_revision:7,document:{elements:[{customData:{planningSource:{version:reviewed.version,based_on_revision:7,document:reviewed.document}}}]}};
checkSketchPlanLifecycle(reviewed,proposed,[sketchRoot]);
for(const patch of [{version:3},{status:'ready'},{proposal_id:'missing'},{updated_at:'2026-11-11T07:59:00Z'},{document:{...reviewed.document,message:'Changed after confirmation'}}])assert.throws(()=>checkSketchPlanLifecycle(reviewed,{...proposed,...patch},[sketchRoot]));
const wrongSnapshot=structuredClone(sketchRoot);wrongSnapshot.document.elements[0].customData.planningSource.version=99;assert.throws(()=>checkSketchPlanLifecycle(reviewed,proposed,[wrongSnapshot]));
assert.throws(()=>checkSketchPlanLifecycle(reviewed,proposed,[{...sketchRoot,parent_proposal_id:'another-root'}]));
console.log('evidence-contract: normal sketch lifecycle accepted; content/version changes, missing or non-root links, old timestamps and wrong snapshots rejected');


for(const separator of [';', '；', ',', '，', '、', '。']) {
 const reference=`Two actually read records (${phosphorylation}${separator}${transport})`;
 assert.deepEqual(evidenceUrls([reference],records),[phosphorylation,transport]);
 const joined=structuredClone(plan);joined.document.edges[0].evidence=[reference];checkEvidencePlan(joined,first,records);
 const unknown=structuredClone(plan);unknown.document.edges[0].evidence=[`${phosphorylation}${separator}https://example.org/unread`];assert.throws(()=>checkEvidencePlan(unknown,first,records),/actually retrieved/);
}
assert.deepEqual(evidenceUrls(['https://example.org/path;a?value=a;b'],[]),['https://example.org/path;a?value=a;b']);
console.log('evidence-contract: adjacent source URLs with prose punctuation split individually; unread adjacent URLs still reject and internal URL semicolons survive');


assert.deepEqual(unreadEvidenceUrls(plan,records),[]);
const unreadPlan=structuredClone(plan);unreadPlan.document.edges[0].evidence.push('https://reactome.org/content/detail/R-HSA-873810');
assert.deepEqual(unreadEvidenceUrls(unreadPlan,records),['https://reactome.org/content/detail/R-HSA-873810']);
assert.throws(()=>checkEvidencePlan(unreadPlan,first,records),/actually retrieved/);
assert.deepEqual(unreadEvidenceUrls(unreadPlan,[...records,{source_url:'https://reactome.org/content/detail/R-HSA-873810',record:{stId:'R-HSA-873810'}}]),[]);
console.log('evidence-contract: unread-source review derives real missing URLs; the original direct-read requirement still rejects unchecked citations');


const graphNotes={elements:[{id:'node',type:'rectangle',x:0,y:240,width:220,height:100,customData:{scientificId:'receptor'}},{id:'edge',type:'arrow',x:100,y:290,width:400,height:120,points:[[0,0],[400,-120]]},{id:'bound-label',type:'text',x:50,y:350,width:100,height:20,containerId:'edge'},{id:'scope',type:'text',x:0,y:380,width:100,height:80},{id:'overlapping',type:'text',x:0,y:360,width:100,height:80}]};
assert.deepEqual(workflowScopeNotes(graphNotes).map(e=>e.id),['scope']);
const rotatedNotes={elements:[{id:'node',type:'rectangle',x:0,y:0,width:100,height:20,angle:Math.PI/2,customData:{scientificId:'receptor'}},{id:'scope',type:'text',x:0,y:61,width:100,height:80},{id:'overlapping',type:'text',x:0,y:40,width:100,height:80}]};
assert.deepEqual(workflowScopeNotes(rotatedNotes).map(e=>e.id),['scope']);
console.log('evidence-contract: note handoff uses actual negative-point arrow bounds, attached text and rotated bounds; overlapping notes remain excluded');

const punctuated=['备选反应 https://reactome.org/content/detail/R-HSA-1；R-HSA-2 另一反应 https://reactome.org/content/detail/R-HSA-2；R-HSA-3 后续说明'];
assert.deepEqual(evidenceUrls(punctuated,[]),['https://reactome.org/content/detail/R-HSA-1','https://reactome.org/content/detail/R-HSA-2']);

const navigationOnly='https://reactome.org/content/detail/R-HSA-99999',withContext=structuredClone(records),contextPlan=structuredClone(plan);
withContext[0].source_context={pathway_context:{status:'available',coverage:'event_ancestor_membership_metadata_only',paths:[[{id:'R-HSA-99999',title:'Ancestor navigation entry'}]]}};contextPlan.document.edges[0].evidence=[navigationOnly];assert.deepEqual(unreadEvidenceUrls(contextPlan,withContext),[navigationOnly],'ancestor membership metadata is not an independently read source');
