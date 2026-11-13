// Protocol/claim-mapping fixtures only; this is not independent biological validation.
import assert from 'node:assert/strict';
import {checkEgfrClarification,checkEgfrPlan,checkEgfrCaptions,checkAggregatedResidueLabels,egfrScenario} from './helpers/canvas-egfr-evidence.mjs';
import {workflowReceptor} from './helpers/canvas-live-evidence-workflow.mjs';
const url=id=>`https://reactome.org/content/detail/${id}`;
const ids=['R-HSA-177942','R-HSA-177943','R-HSA-177938','R-HSA-5672969','R-HSA-5672978','R-HSA-5672973'];
const records=ids.map(stId=>({source_url:url(stId),record:{stId,schemaClass:'Reaction'}}));
const first={status:'needs_clarification',version:1,document:{questions:[{id:'species',question:'物种？',answer:'',blocking:true},{id:'variant',question:'野生型还是变体？',answer:'',blocking:true},{id:'endpoint',question:'终点？',answer:'',blocking:true}]}};
checkEgfrClarification(first,[{name:'figure_evidence',input:{action:'search',query:'EGFR ERK'},is_error:false}]);
const labels=['EGF','野生型 EGFR','GRB2/SOS1','RAS-GTP','活化 RAF 家族','磷酸化 MEK1/2','活化 ERK1/2'];
const plan={status:'ready',version:2,document:{message:'人类野生型 EGF 到 ERK 活化概念图',assumptions:['RAS/RAF/MEK/ERK 使用家族层级抽象，省略部分活化中间步骤，不指定唯一亚型。'],nodes:labels.map((label,i)=>({id:String(i),label})),edges:ids.map((id,i)=>({from:String(i),to:String(i+1),label:'Declared abstract transition',evidence:[url(id)]})),questions:first.document.questions.map((q,i)=>({...q,answer:['人类','野生型 EGFR','止于 ERK 活化'][i]}))}};
checkEgfrPlan(plan,first,records);checkEgfrCaptions('人类野生型 EGF-EGFR 到活化 ERK，RAF/MEK/ERK 按家族概括，不展开核内转录。');
for(const label of ['RAS-GTP 招募并磷酸化 RAF（跨面板）','RAS directly phosphorylates RAF','RAS-GTP recruits and phosphorylates RAF']){
 const invalid=structuredClone(plan);invalid.document.edges[3].label=label;
 assert.throws(()=>checkEgfrPlan(invalid,first,records),/actual kinase attribution/);
}
for(const label of ['RAS-GTP 招募 RAF；RAF 激活激酶磷酸化 RAF','RAS-dependent RAF recruitment and activation','RAF 经其他激酶磷酸化而活化']){
 const valid=structuredClone(plan);valid.document.edges[3].label=label;checkEgfrPlan(valid,first,records);
}
assert.equal(workflowReceptor(plan,egfrScenario.receptorPattern,egfrScenario.receptorExclude).id,'1');
for(const mutate of [p=>p.document.questions[0].blocking=false,p=>{p.document.edges[4].evidence=[url(ids[5])];p.document.edges[5].evidence=[url(ids[4])];},p=>p.document.edges.splice(3,1),p=>p.document.nodes[1].label='EGFR L858R',p=>p.document.nodes[6].label='Activated MAPK14 (p38)',p=>p.document.assumptions=[],p=>p.document.questions.pop(),p=>p.document.edges[0].evidence.push('https://pubmed.ncbi.nlm.nih.gov/999999/')]){const invalid=structuredClone(plan);mutate(invalid);assert.throws(()=>checkEgfrPlan(invalid,first,records));}
const overview=structuredClone(records);overview[0].record.schemaClass='Pathway';assert.throws(()=>checkEgfrPlan(plan,first,overview));
assert.throws(()=>checkEgfrPlan(plan,first,records.slice(1)));
assert.throws(()=>checkEgfrCaptions('EGF ERK'));
for(const scope of ['人细胞中','人源细胞中','人体细胞中','人类细胞中'])checkEgfrCaptions(`${scope} EGF 刺激野生型 EGFR，经 RAS/RAF/MEK/ERK 家族概括至活化 ERK1/2。`);
for(const scope of ['小鼠细胞中','细胞中','非人细胞中','非人源细胞中'])assert.throws(()=>checkEgfrCaptions(`${scope} EGF 刺激野生型 EGFR，经 RAS/RAF/MEK/ERK 家族概括至活化 ERK1/2。`));
assert.throws(()=>checkEgfrClarification(first,[{name:'figure_evidence',input:{action:'search',species:'Homo sapiens'},is_error:false}]));
for(const prompt of [egfrScenario.initialRequest,egfrScenario.confirmation,egfrScenario.captionPrompt])assert.doesNotMatch(prompt,/R-HSA-\d+/,'reference oracle IDs are not supplied to the Agent');
console.log('EGFR evidence: unanswered scope, retained answers, reaction/transition mapping, directed path, family abstraction, wrong source/mutant/p38 negatives and private oracle boundaries passed');

const isoformRecords=[...records,...['R-HSA-109860','R-HSA-109862'].map(stId=>({source_url:url(stId),record:{stId,schemaClass:'Reaction'}}))];
const isoformPlan=structuredClone(plan);isoformPlan.document.edges[5].evidence=['R-HSA-109860','R-HSA-109862'].map(url);
checkEgfrPlan(isoformPlan,first,isoformRecords);
for(const missing of [0,1]){const incomplete=structuredClone(isoformPlan);incomplete.document.edges[5].evidence.splice(missing,1);assert.throws(()=>checkEgfrPlan(incomplete,first,isoformRecords));}
const misplaced=structuredClone(isoformPlan);misplaced.document.edges[4].evidence.push(misplaced.document.edges[5].evidence.pop());assert.throws(()=>checkEgfrPlan(misplaced,first,isoformRecords));

const endpointParaphrase=structuredClone(first);endpointParaphrase.document.questions[2].question='ERK 之后是否继续展开，还是到 ERK 活性即止？';
checkEgfrClarification(endpointParaphrase,[{name:'figure_evidence',input:{action:'search'},is_error:false}]);
endpointParaphrase.document.questions[2].question='ERK 配色用蓝色还是绿色？';assert.throws(()=>checkEgfrClarification(endpointParaphrase,[{name:'figure_evidence',input:{action:'search'},is_error:false}]));

for(const label of ['ERK1/2 · p-T202/Y204','MEK1/2 · p-S218/S222','MAPK3/MAPK1 pT202/Y204'])assert.throws(()=>checkAggregatedResidueLabels([{label}]));
for(const label of ['磷酸化 ERK1/2（p-2T MAPK，质膜）终点','ERK1/2 双苏氨酸磷酸化','ERK two threonines phosphorylated'])assert.throws(()=>checkAggregatedResidueLabels([{label}]),/two threonines/);
for(const label of ['磷酸化 ERK1/2（活化态）','ERK1/2（活化环 Thr/Tyr 磷酸化）'])checkAggregatedResidueLabels([{label}]);
for(const label of ['磷酸化 ERK1/2','活化 MEK1/2','ERK1 p-T202/Y204','ERK1/2（ERK1 T202/Y204；ERK2 T185/Y187）','MEK1/2（MEK1 S218/S222；MEK2 S222/S226）'])checkAggregatedResidueLabels([{label}]);

// Residue attribution must remain clear when the numbers are on an edge.
const residueNodes=[{id:"mek",label:"MEK 磷酸化（MAP2K1/MAP2K2）"},{id:"erk",label:"ERK1/2 磷酸化活化（MAPK3/MAPK1）"}];
assert.throws(()=>checkAggregatedResidueLabels(residueNodes,[{to:"mek",label:"磷酸化 MEK 活化环双丝氨酸（S218/S222、S222/S226）"}]),/assigned explicitly/);
assert.throws(()=>checkAggregatedResidueLabels(residueNodes,[{to:"erk",label:"磷酸化 ERK 活化环 Thr/Tyr（T202/Y204、T185/Y187）"}]),/assigned explicitly/);
checkAggregatedResidueLabels(residueNodes,[{to:"mek",label:"磷酸化：MEK1 S218/S222；MEK2 S222/S226"},{to:"erk",label:"磷酸化：ERK1 T202/Y204；ERK2 T185/Y187"}]);
checkAggregatedResidueLabels(residueNodes,[{to:"mek",label:"磷酸化 MEK 活化环"},{to:"erk",label:"磷酸化 ERK 活化环 Thr/Tyr"}]);
