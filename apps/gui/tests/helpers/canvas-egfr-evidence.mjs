// Independent checks from Reactome EGFR/RAF-MAPK records, not an Agent-supplied answer.
// Oracle IDs below never appear in the generation/confirmation prompts.
import assert from 'node:assert/strict';
import {evidenceUrls,checkAbstractCoverage} from './canvas-live-evidence.mjs';
const species=q=>/物种|species|organism|人类|小鼠/i.test(q.question);
const variant=q=>/野生型|突变|变体|wild.?type|mutant|variant/i.test(q.question);
const endpoint=q=>/终点|endpoint|入核|终止|到哪|是否继续展开|(?:到|止于).{0,16}(?:活性|活化|激活|为止|即止)/i.test(q.question);
const ras=s=>/\b[HKN]?RAS\b|RAS[-\s]?GTP/i.test(s);
const raf=s=>/\b[ABC]?RAF1?\b|RAF家族/i.test(s);
const mek=s=>/MEK[12]?(?:\/[12])?\b|MAP2K[12]\b/i.test(s);
const erk=s=>/ERK[12]?(?:\/[12])?\b|MAPK[13]\b/i.test(s);
const receptor=s=>/EGFR|表皮.*受体/i.test(s);
const abstraction=s=>/家族|family|亚型|isoform|简化|抽象/i.test(s)&&/RAS|RAF|MEK|ERK|MAP2K|MAPK/i.test(s);
const humanScope=/(?<!非)(?:人类|人(?:源|体)?细胞)|\bhuman\b|Homo sapiens/i;
export function checkEgfrClarification(first,calls){
 assert.equal(first.status,'needs_clarification');const questions=first.document.questions.filter(q=>q.blocking!==false);
 assert.ok(questions.length<=3);assert.ok(questions.some(species)&&questions.some(variant)&&questions.some(endpoint),'organism, wild-type/variant and endpoint remain scientific questions');
 for(const q of questions)assert.equal(q.answer.trim(),'');
 const searches=calls.filter(c=>!c.is_error&&c.name==='figure_evidence'&&c.input.action==='search');assert.ok(searches.length);assert.equal(searches[0].input.species??null,null,'initial search does not invent a species');
}
// Attribution clarity only: actual site correctness still needs source-level review.
export function checkAggregatedResidueLabels(nodes,edges=[]){
 const byId=new Map(nodes.map(node=>[node.id,node]));
 // Edge labels also carry residue numbers. Inspect them with their declared target
 // family, rather than accepting an abstract node while missing an ambiguous edge.
 const labels=[...nodes,...edges.map(edge=>({label:`${byId.get(edge.to)?.label??""} ${edge.label??""}`}))];
 for(const node of labels)if(erk(node.label))assert.ok(!/p\s*[-–]?\s*2T\b|双苏氨酸|two\s+threonines/i.test(node.label),'ERK activation-loop shorthand must not imply two threonines when the curated mechanism specifies threonine and tyrosine; resolve conflicting source names or keep the state abstract');
 const numbered=/(?:p[- ]?)?(?:Thr|Ser|Tyr|[STY])\s*\d{2,}/i;
 const families=[{group:/ERK1\s*\/\s*(?:ERK)?2|MAPK1\s*\/\s*(?:MAPK)?3|MAPK3\s*\/\s*(?:MAPK)?1/gi,members:[/(?:ERK1|MAPK3)\s*[:：（( -]*\s*(?:p[- ]?)?(?:Thr|Ser|Tyr|[STY])\s*\d{2,}/i,/(?:ERK2|MAPK1)\s*[:：（( -]*\s*(?:p[- ]?)?(?:Thr|Ser|Tyr|[STY])\s*\d{2,}/i]}, {group:/MEK1\s*\/\s*(?:MEK)?2|MAP2K1\s*\/\s*(?:MAP2K)?2/gi,members:[/(?:MEK1|MAP2K1)\s*[:：（( -]*\s*(?:p[- ]?)?(?:Thr|Ser|Tyr|[STY])\s*\d{2,}/i,/(?:MEK2|MAP2K2)\s*[:：（( -]*\s*(?:p[- ]?)?(?:Thr|Ser|Tyr|[STY])\s*\d{2,}/i]}];
 for(const node of labels)for(const family of families){const stripped=node.label.replace(family.group,'__FAMILY__');if(stripped.includes('__FAMILY__')&&numbered.test(node.label))assert.ok(family.members.every(member=>member.test(stripped)),'aggregated family residue numbers must be assigned explicitly to each member; a caption caveat does not fix an ambiguous label');}
}
// Bounded attribution regression, not a general biochemical language parser.
export function checkRafCatalystAttribution(nodes,edges){
 const index=new Map(nodes.map(node=>[node.id,node]));
 for(const edge of edges)if(ras(index.get(edge.from)?.label??'')&&raf(index.get(edge.to)?.label??'')){
  const explicitRasKinase=/RAS(?:[-:\s]?GTP)?\s*(?:(?:招募|并|直接|催化|同时|和|及)\s*)*磷酸化\s*RAF|RAS(?:[-:\s]?GTP)?\s+(?:(?:directly|recruits?|and)\s+)*phosphorylates?\s+RAF/i;
  assert.ok(!explicitRasKinase.test(edge.label),'RAS recruitment does not establish RAS as the RAF phosphorylation catalyst; preserve the actual kinase attribution');
 }
}
export function checkEgfrPlan(plan,first,records){
 assert.equal(plan.status,'ready');assert.ok(plan.version>first.version);
 for(const question of first.document.questions){const current=plan.document.questions.find(q=>q.id===question.id);assert.ok(current,'original question identity retained');if(question.blocking!==false){assert.notEqual(current.blocking,false);assert.ok(current.answer.trim());}}
 const {nodes,edges}=plan.document;checkAggregatedResidueLabels(nodes,edges);checkRafCatalystAttribution(nodes,edges);assert.ok(nodes.length>=5&&nodes.length<=8);assert.ok(edges.length>=4);
 const scientificText=[plan.document.message,...plan.document.assumptions,...nodes.map(n=>n.label),...plan.document.questions.map(q=>q.answer)].join('\n');
 assert.match(scientificText,/人类|human|Homo sapiens/i);assert.match(scientificText,/野生型|wild.?type|\bWT\b/i);assert.ok(abstraction([plan.document.message,...plan.document.assumptions].join('\n')),'family aggregation and abstraction are declared');
 const labels=nodes.map(n=>n.label);assert.ok(labels.some(receptor)&&labels.some(ras)&&labels.some(raf)&&labels.some(mek)&&labels.some(erk));assert.match(labels.join(' '),/GRB2|SOS1/i);
 assert.ok(!labels.some(s=>/EGFRvIII|L858R|V600E|G12[CDV]|STAT1|IFNGR/i.test(s)),'do not replace the wild-type EGF-MAPK case with mutants or IFNG');
 const index=new Map(nodes.map(n=>[n.id,n]));
 for(const edge of edges){assert.ok(index.has(edge.from)&&index.has(edge.to));checkAbstractCoverage(edge.evidence,records);const cited=evidenceUrls(edge.evidence,records);assert.ok(cited.length);for(const url of cited)assert.ok(records.some(r=>r.source_url===url),`unread EGFR source ${url}`);assert.ok(cited.some(url=>records.some(r=>r.source_url===url&&/^R-HSA-/.test(r.record?.stId??'')&&/Reaction|BlackBoxEvent|Polymerisation/i.test(r.record?.schemaClass??''))),'each declared transition has a read reaction/event, not merely an overview or entity');}
 const linked=(ids,from,to)=>edges.some(edge=>from(index.get(edge.from).label)&&to(index.get(edge.to).label)&&records.some(record=>ids.includes(record.record?.stId)&&evidenceUrls(edge.evidence,records).includes(record.source_url)));
 assert.ok(linked(['R-HSA-177938','R-HSA-177945'],s=>/GRB2|SOS1|EGFR/i.test(s),ras),'EGFR-associated SOS1 exchange source belongs to the RAS activation transition');
 assert.ok(linked(['R-HSA-5672978'],raf,mek),'RAF-to-MEK cites the MAP2K phosphorylation reaction');
 const erkReaction=linked(['R-HSA-5672973'],mek,erk)||edges.some(edge=>mek(index.get(edge.from).label)&&erk(index.get(edge.to).label)&&['R-HSA-109860','R-HSA-109862'].every(id=>records.some(record=>record.record?.stId===id&&evidenceUrls(edge.evidence,records).includes(record.source_url))));
 assert.ok(erkReaction,'MEK-to-ERK cites the aggregate MAPK phosphorylation reaction or both read MEK1/ERK1 and MEK2/ERK2 reactions on that transition');
 const starts=nodes.filter(n=>/EGF|表皮/i.test(n.label)&&!erk(n.label)),ends=nodes.filter(n=>erk(n.label)&&/活化|磷酸|activ|phosph|\bp[- ]/i.test(n.label));assert.ok(starts.length&&ends.length,'ligand/receptor source and active ERK endpoint are explicit');
 const reached=new Set(starts.map(n=>n.id));let changed=true;while(changed){changed=false;for(const e of edges)if(reached.has(e.from)&&!reached.has(e.to)){reached.add(e.to);changed=true;}}
 assert.ok(ends.some(n=>reached.has(n.id)),'declared signal reaches active ERK');
}
export function checkEgfrCaptions(text){assert.match(text,humanScope);assert.match(text,/EGF|表皮/i);assert.match(text,/ERK|MAPK[13]/i);assert.ok(abstraction(text),'family abstraction remains visibly reviewable');assert.match(text,/野生型|wild.?type|\bWT\b/i);}
export const egfrScenario={
 name:'egfr-mapk',targetName:'EGFR',receptorPattern:/EGFR|表皮.*受体/i,receptorExclude:/GRB2|SOS1|RAS|RAF|MEK|ERK|MAPK/i,
 initialRequest:'我准备给研究生画 EGF 刺激后经 EGFR 到 ERK 的科研示意图，但还没有确定物种、使用野生型 EGFR 还是肿瘤变体，以及 ERK 之后是否继续展开。请先轻量检索 Reactome 候选来源，不默认物种或变体，把已知图意和最多三个必须澄清的问题保存到计划，保留空答案。本轮不画图、不生成候选、不添加素材。',
 confirmation:'确认：物种是人类，配体为 EGF，使用野生型 EGFR，从 EGF/EGFR 结合与活化开始，经 GRB2/SOS1、RAS、RAF、MEK 到 ERK1/2 活化为止，不展开入核转录、PI3K/AKT、药物、肿瘤变体或反馈支路。至多八个生物组件，可以合并复合物；RAS/RAF/MEK/ERK 可按家族层级概括，但须说明这种抽象，不能把家族等同某个唯一亚型或略过重要机制。自行检索并读取相关反应级记录，逐条保留实际来源 URL，把关键抽象与不确定性写入计划；不要以一个通路总览证明所有箭头。保留并回答原问题，保存供我确认的就绪计划。本轮不画图或添加素材，缺失来源就说明，不猜测。',
 captionPrompt:'图下添加可编辑范围说明，明确人类野生型 EGF–EGFR 到活化 ERK 的终点、家族层级简化及关键抽象，不将这些限定仅藏在元数据。',
 checkClarification:checkEgfrClarification,checkPlan:checkEgfrPlan,checkCaptions:checkEgfrCaptions,
 scope:'same-session human wild-type EGF-EGFR-RAS-RAF-MEK-ERK source discovery, confirmed diagram and generic receptor artwork; family abstraction and source coverage checked, scientific correctness requires review',
};
