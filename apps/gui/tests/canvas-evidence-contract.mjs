import assert from 'node:assert/strict';
import {checkEvidencePlan,checkEvidenceClarification,checkEvidenceExport} from './helpers/canvas-live-evidence.mjs';
import {relationSources} from '../src/features/canvas/canvas-source-manifest.ts';
const first={version:1,document:{questions:[{id:'species',question:'Which species?',answer:''},{id:'subtype',question:'Which interferon subtype?',answer:''}]}};
const phosphorylation='https://reactome.org/content/detail/R-HSA-873922',transport='https://reactome.org/content/detail/R-HSA-873917',overview='https://reactome.org/content/detail/R-HSA-877300';
const records=[['R-HSA-873922',phosphorylation],['R-HSA-873917',transport],['R-HSA-877300',overview]].map(([id,url])=>({record:{stId:id},source_url:url}));
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
