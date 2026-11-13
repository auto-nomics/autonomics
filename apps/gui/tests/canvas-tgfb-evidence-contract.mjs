import assert from 'node:assert/strict';
import {checkTgfbClarification,checkTgfbPlan,checkTgfbCaptions,tgfbScenario} from './helpers/canvas-tgfb-evidence.mjs';
const first={version:1,status:'needs_clarification',document:{questions:[{id:'species',question:'物种？',answer:''},{id:'ligand',question:'配体亚型？',answer:''}]}};
checkTgfbClarification(first,[{name:'figure_evidence',input:{action:'search'},is_error:false}]);
const ids=['R-HSA-170861','R-HSA-170843','R-HSA-170868','R-HSA-170847','R-HSA-173488'];
const records=ids.map(stId=>({source_url:`https://reactome.org/content/detail/${stId}`,record:{stId,schemaClass:'Reaction',summation:[]}}));
const cite=(...indices)=>indices.map(i=>records[i].source_url);
const nodes=[{id:'ligand',label:'TGFB1 胞外配体'},{id:'receptor',label:'活化 TGFBR1/TGFBR2 受体复合物'},{id:'rsmad',label:'磷酸化 SMAD2/SMAD3'},{id:'complex',label:'SMAD2/SMAD3:SMAD4 复合物（胞质）'},{id:'nuclear',label:'SMAD2/SMAD3:SMAD4 复合物（细胞核）'}];
const edges=[{from:'ligand',to:'receptor',label:'结合与TGFBR1活化',evidence:cite(0,1)},{from:'receptor',to:'rsmad',label:'TGFBR1 磷酸化 SMAD2/3',evidence:cite(2)},{from:'rsmad',to:'complex',label:'与SMAD4形成复合物',evidence:cite(3)},{from:'complex',to:'nuclear',label:'复合物入核',evidence:cite(4)}];
const plan={version:2,status:'ready',document:{message:'人类野生型TGFB1机制',nodes,edges,assumptions:['早期内体过程作简化，不展开下游转录。'],questions:first.document.questions.map(q=>({...q,answer:q.id==='species'?'人类':'TGFB1'}))}};
checkTgfbPlan(plan,first,records);
for(const mutate of [p=>p.document.questions.pop(),p=>p.document.nodes[4].label='STAT1 细胞核',p=>p.document.edges[1].label='TGFBR2 直接磷酸化 SMAD2',p=>p.document.edges[3].evidence=cite(3),p=>p.document.edges[2].evidence=['https://reactome.org/content/detail/unread'],p=>p.document.edges[3].to='complex']){const bad=structuredClone(plan);mutate(bad);assert.throws(()=>checkTgfbPlan(bad,first,records));}
const conflict=structuredClone(records);conflict[2].record.summation=[{text:'SMAD1, SMAD5 or SMAD8'}];assert.throws(()=>checkTgfbPlan(plan,first,conflict));
const reviewed=structuredClone(plan);reviewed.document.assumptions.push('该反应摘要与参与物存在SMAD分支不一致，保留冲突，图示以已核对的参与物为限。');checkTgfbPlan(reviewed,first,conflict);
checkTgfbCaptions('人类野生型TGFB1经SMAD2/3–SMAD4入核，早期内体过程简化。');assert.throws(()=>checkTgfbCaptions('TGF信号图'));
for(const prompt of [tgfbScenario.initialRequest,tgfbScenario.confirmation,tgfbScenario.captionPrompt])assert.doesNotMatch(prompt,/R-HSA-\d+|https?:\/\/reactome/,'oracle source IDs are never supplied as generation answers');
console.log('TGF-beta evidence contract: source attribution, scope clarification, nuclear endpoint, source conflict and preserved questions passed');

const wrongSpecies=structuredClone(records);wrongSpecies[0].record.stId='R-MMU-170861';assert.throws(()=>checkTgfbPlan(plan,first,wrongSpecies));

assert.throws(()=>checkTgfbCaptions('人类野生型TGFB1经SMAD2/3–SMAD4入核，早期内体过程简化。',conflict));
checkTgfbCaptions('人类野生型TGFB1经SMAD2/3–SMAD4入核，过程简化；该来源摘要与参与物不一致。',conflict);

checkTgfbCaptions('人类野生型TGFB1经SMAD2/3–SMAD4入核，过程简化。该记录标题与参与物为SMAD2/3，但其摘要文字写的是SMAD1/5/8，本图按参与物采用SMAD2/3。',conflict);
assert.throws(()=>checkTgfbCaptions('人类野生型TGFB1经SMAD2/3–SMAD4入核，过程简化；摘要包含SMAD1/5/8。',conflict));
