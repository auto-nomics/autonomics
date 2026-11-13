// Bounded regression checks against independently retrieved Reactome reactions.
// Oracle IDs are never included in the Agent's generation prompts.
import assert from 'node:assert/strict';
import {evidenceUrls,checkAbstractCoverage} from './canvas-live-evidence.mjs';
const cycs=s=>/CYCS|cytochrome\s*c|细胞色素\s*[cＣ]/i.test(s);
const apaf=s=>/APAF[-‐‑–—\s]?1/i.test(s);
const casp9=s=>/CASP(?:ASE)?[-‐‑–—\s]?9/i.test(s);
const casp3=s=>/CASP(?:ASE)?[-‐‑–—\s]?3/i.test(s);
const human=s=>/人类|Homo sapiens|\bhuman\b/i.test(s);
export function checkCasp9ProcessingQualification(text){
 assert.ok(casp9(text),'CASP9 qualification names its subject');
 assert.match(text,/二聚|dimer/i,'CASP9 activation abstraction includes dimerization');
 assert.match(text,/切割|剪切|加工|裂解|cleav|process/i,'processing is distinguished from activation');
 assert.match(text,/非必要|非必需|非必须|不必要|无需|不依赖.{0,12}(?:加工|剪切|切割)|并非(?:\s*CASP(?:ASE)?[-‐‑–—\s]?9\s*)?催化活性所(?:必需|必要)|并非.{0,12}(必需|必要)|不是.{0,12}(必需|必要)|不.{0,12}(需要|必需|必要)|not.{0,25}(requir|necess)|dispensable/i,'processing is not presented as an obligatory activation prerequisite');
}
export function checkApoptosisClarification(plan,calls){
 assert.equal(plan.status,'needs_clarification');
 const questions=plan.document.questions.filter(q=>q.blocking!==false);assert.ok(questions.length>0&&questions.length<=3);
 assert.ok(questions.some(q=>/物种|species|organism/i.test(q.question)));
 assert.ok(questions.some(q=>/范围|起点|终点|到哪|终止|scope|start|endpoint|展开/i.test(q.question)));
 for(const question of questions)assert.equal(question.answer.trim(),'');
 const searches=calls.filter(c=>!c.is_error&&c.name==='figure_evidence'&&c.input.action==='search');assert.ok(searches.length>0);assert.equal(searches[0].input.species??null,null,'no species invented before clarification');
}
function unsupportedCasp3Subunits(nodes,records){
 const output=records.filter(r=>r.record?.stId==='R-HSA-114252').map(r=>JSON.stringify(r.record?.output??[])).join(' ').toLowerCase();
 return nodes.filter(node=>casp3(node.label)).flatMap(node=>node.label.match(/p(?:20|10)\b/gi)??[]).filter(name=>!output.includes(name.toLowerCase()));
}
export function reviewApoptosisPlan(plan,records){
 const cited=plan.document.edges.flatMap(edge=>evidenceUrls(edge.evidence,records));
 const missingDimer=!records.some(record=>record.record?.stId==='R-HSA-9627056')||!cited.includes('https://reactome.org/content/detail/R-HSA-9627056');
 const unsupportedSubunits=unsupportedCasp3Subunits(plan.document.nodes,records).length>0;
 if(!missingDimer&&!unsupportedSubunits)return null;
 return [
  '科学计划审阅：保留已确认的人类内源性范围、原问题ID/答案和已有实际来源，本轮仍不画图、不生成候选、不添加素材。',
  ...(missingDimer?['请重新核对procaspase-9招募之后的活化机制。当前关系引用没有覆盖二聚活化的反应：不能只用“加工不必需”的文字说明代替该转变。请在已发现的凋亡小体相关通路中用events导航并实际读取合适反应，核对输入/输出复合物和区室；允许合并中间态，但合并后的关系标签与依据须明确包含实际支持的活化步骤。保留二聚活化与蛋白水解加工的区别，不把加工写成催化活性的必要条件。']:[]),
  ...(unsupportedSubunits?['CASP3标签中的亚基简称尚未由本次读取的反应产物名称核对。请对照实际CASP3产物记录逐项检查，不从其他caspase类推亚基简称；图是机制抽象，可以保留粗粒度活性状态而不新增结构或编号。若保留亚基或残基信息，须有实际读到且对应同一分子的明确依据。']:[]),
  '不要把搜索无命中或未读到某条记录表述为不存在。逐关系保留实际来源URL，删除无法核对的额外简称，更新并保存就绪计划供再次审阅。'
 ].join('\n');
}
export function checkApoptosisPlan(plan,first,records){
 assert.equal(plan.status,'ready');assert.ok(plan.version>first.version);
 for(const original of first.document.questions){const question=plan.document.questions.find(q=>q.id===original.id);assert.ok(question);if(original.blocking!==false){assert.notEqual(question.blocking,false);assert.ok(question.answer.trim());}}
 const {nodes,edges}=plan.document;assert.ok(nodes.length>=5&&nodes.length<=8);assert.ok(edges.length>=4);
 const labels=nodes.map(n=>n.label),description=[plan.document.message,...plan.document.assumptions,...labels,...plan.document.questions.map(q=>q.answer)].join('\n');
 assert.ok(human(description));for(const predicate of [cycs,apaf,casp9,casp3])assert.ok(labels.some(predicate));
 assert.match(labels.join(' '),/膜间隙|intermembrane/i);assert.match(labels.join(' '),/胞质|cytosol/i);
 assert.ok(!labels.some(s=>/IFNGR|STAT1|EGFR|TNFR|FAS|CASP(?:ASE)?[-\s]?(?:7|8)\b|XIAP|SMAC|DIABLO/i.test(s)),'excluded branches do not enter the graph');
 checkCasp9ProcessingQualification([plan.document.message,...plan.document.assumptions].join('\n'));
 assert.deepEqual(unsupportedCasp3Subunits(nodes,records),[],'unverified CASP3 subunit shorthand must not be inferred from other caspases');
 const index=new Map(nodes.map(n=>[n.id,n])),cited=new Set();
 for(const edge of edges){
  assert.ok(index.has(edge.from)&&index.has(edge.to));checkAbstractCoverage(edge.evidence,records);
  const urls=evidenceUrls(edge.evidence,records);assert.ok(urls.length>0);
  for(const url of urls){assert.ok(records.some(r=>r.source_url===url),`unread apoptosis source ${url}`);cited.add(url);}
  assert.ok(urls.some(url=>records.some(r=>r.source_url===url&&/^R-HSA-/.test(r.record?.stId??'')&&/Reaction|BlackBoxEvent/i.test(r.record?.schemaClass??''))),'each transition cites an actually read human reaction/event');
 }
 for(const id of ['114284','114254','114256','9627056','114252'])assert.ok(cited.has(`https://reactome.org/content/detail/R-HSA-${id}`),`retain the actual release/assembly/recruitment/dimerization/execution source ${id}`);
 const ends=nodes.filter(n=>casp3(n.label)&&/活化|活性|激活|成熟|active|activat|mature/i.test(n.label));assert.ok(ends.length>0,'endpoint is visibly active CASP3');
 assert.ok(edges.some(e=>ends.some(n=>n.id===e.to)&&evidenceUrls(e.evidence,records).includes('https://reactome.org/content/detail/R-HSA-114252')),'CASP3 transition carries its cleavage reaction source');
 const reached=new Set(nodes.filter(n=>cycs(n.label)&&/膜间隙|intermembrane/i.test(n.label)).map(n=>n.id));assert.ok(reached.size>0);
 let changed=true;while(changed){changed=false;for(const edge of edges)if(reached.has(edge.from)&&!reached.has(edge.to)){reached.add(edge.to);changed=true;}}
 assert.ok(ends.some(n=>reached.has(n.id)),'a declared directed path reaches active CASP3 from mitochondrial CYCS');
}
export function checkApoptosisCaptions(text){assert.ok(human(text));assert.match(text,/内源|intrinsic/i);assert.ok(casp3(text));checkCasp9ProcessingQualification(text);}
export const apoptosisScenario={
 name:'intrinsic-apoptosis',
 initialRequest:'我准备给研究生画线粒体内源性凋亡机制示意图，但还没确定物种和具体起止范围。请轻量检索 Reactome 候选来源，不默认物种，提出最多三个必要澄清问题。本轮不要画图、生成候选或添加素材。',
 confirmation:'确认：物种为人类。起点为线粒体膜间隙的细胞色素c（CYCS）释放，随后展示胞质中的APAF1/凋亡小体组装、procaspase-9招募与活化，终点为CASP3活化。不展开上游BCL2家族调控、死亡受体、CASP8、CASP7、XIAP/SMAC或下游底物降解。5至8个生物组件，可以合并有明确来源支持的复合物或中间态，保持区室清楚；图是机制抽象而非分子结构。请自行检索并实际读取反应级记录，区分结合、组装、活化和蛋白水解加工，保留来源限定和关键抽象；不能用通路总览证明所有箭头。逐关系保存实际来源URL，保留上一轮问题ID并填写答案，保存就绪计划供我确认。本轮仍不画图或选素材，缺少来源就说明，不猜测。',
 captionPrompt:'图下添加可编辑范围说明，明确人类内源性分支、CYCS释放起点和CASP3活化终点，保留已确认计划中CASP9活化与加工的来源限定和机制抽象，不仅藏在元数据。',
 reviewPlan:reviewApoptosisPlan,checkClarification:checkApoptosisClarification,checkPlan:checkApoptosisPlan,checkCaptions:checkApoptosisCaptions,
 scope:'same-session human intrinsic apoptosis source discovery, scope clarification, source-supported editable sketch and review; excludes upstream regulation and downstream substrate processing; no universal scientific correctness claim',
};
