// Independent bounded Reactome oracle. IDs below never enter generation prompts.
import assert from 'node:assert/strict';
import {evidenceUrls,checkAbstractCoverage} from './canvas-live-evidence.mjs';
const smad=s=>/SMAD\s*2\s*[/,、和及]?\s*(?:SMAD\s*)?3|SMAD2|SMAD3/i.test(s);
const complex=s=>smad(s)&&/SMAD4/i.test(s);
const nuclear=s=>complex(s)&&/核|nucle/i.test(s);
const canonical=['R-HSA-170843','R-HSA-170868','R-HSA-170847','R-HSA-173488'];
export function checkTgfbClarification(plan,calls){
 assert.equal(plan.status,'needs_clarification');const required=plan.document.questions.filter(q=>q.blocking!==false);assert.ok(required.length>0&&required.length<=3);
 for(const q of required)assert.equal(q.answer.trim(),'');
 assert.ok(required.some(q=>/物种|species|organism/i.test(q.question)));
 assert.ok(required.some(q=>/配体|亚型|TGFB1|TGF.{0,6}1|isoform|ligand/i.test(q.question)));
 const searches=calls.filter(c=>!c.is_error&&c.name==='figure_evidence'&&c.input.action==='search');assert.ok(searches.length);assert.equal(searches[0].input.species??null,null);
}
export function checkTgfbPlan(plan,first,records){
 assert.equal(plan.status,'ready');assert.ok(plan.version>first.version);
 for(const old of first.document.questions){const q=plan.document.questions.find(q=>q.id===old.id);assert.ok(q);if(old.blocking!==false){assert.notEqual(q.blocking,false);assert.ok(q.answer.trim());}}
 const {nodes,edges}=plan.document;assert.ok(nodes.length>=5&&nodes.length<=8);assert.ok(edges.length>=4);
 const text=[plan.document.message,...plan.document.assumptions,...nodes.map(n=>n.label),...edges.map(e=>e.label??''),...plan.document.questions.map(q=>q.answer)].join('\n');
 assert.match(text,/人类|Homo sapiens|human/i);assert.match(text,/TGFB1|TGF[-‐‑–—\s]?[βb][ -]?1/i);assert.match(text,/野生型|wild.?type|\bWT\b/i);
 assert.match(text,/早期内体|early endosom/i);assert.match(text,/简化|抽象|合并|abstract|simplif/i);
 const labels=nodes.map(n=>n.label).join(' ');assert.match(labels,/TGFBR1/i);assert.match(labels,/TGFBR2/i);assert.ok(nodes.some(n=>smad(n.label)));assert.ok(nodes.some(n=>nuclear(n.label)));
 assert.doesNotMatch(labels,/SMAD[1589]\b|EGFR|IFNGR|STAT1|CASP(?:ASE)?\s*\d|BMP|ACTIVIN|NODAL/i,'do not substitute another branch or earlier test figure');
 const index=new Map(nodes.map(n=>[n.id,n]));
 for(const edge of edges){assert.ok(index.has(edge.from)&&index.has(edge.to));checkAbstractCoverage(edge.evidence,records);const urls=evidenceUrls(edge.evidence,records);assert.ok(urls.length);for(const url of urls){const record=records.find(r=>r.source_url===url);assert.ok(record,'no unread source');if(record.record?.stId?.startsWith('R-'))assert.match(record.record.stId,/^R-HSA-/,'confirmed human scope cannot silently use a nonhuman Reactome reaction');}assert.ok(urls.some(url=>records.some(r=>r.source_url===url&&/Reaction|BlackBoxEvent|Polymerisation/.test(r.record?.schemaClass??''))),'transition has read event evidence');assert.doesNotMatch(edge.label??'',/TGFBR2\s*(?:直接|催化)?\s*(?:磷酸化|phosphorylates?)\s*SMAD[23]/i,'do not assign R-SMAD catalysis directly to TGFBR2');}
 for(const id of canonical){const record=records.find(r=>r.record?.stId===id);assert.ok(record,`read essential event ${id}`);assert.ok(edges.some(e=>evidenceUrls(e.evidence,records).includes(record.source_url)),`link essential event ${id}`);}
 const phosphorylation=records.find(r=>r.record?.stId==='R-HSA-170868');
 const summary=JSON.stringify(phosphorylation?.record?.summation??[]);
 if(['SMAD1','SMAD5','SMAD8'].every(name=>summary.includes(name))){const caveats=plan.document.assumptions.join(' ');assert.match(caveats,/冲突|不一致|不符|conflict|inconsisten/i,'observed source conflict is retained in the scientific plan');assert.match(caveats,/摘要|summar|summation/i);}
 const transport=edges.filter(e=>nuclear(index.get(e.to).label)&&!nuclear(index.get(e.from).label));assert.ok(transport.length);for(const edge of transport)assert.ok(evidenceUrls(edge.evidence,records).includes('https://reactome.org/content/detail/R-HSA-173488'),'nuclear transport evidence belongs to actual transport transition');
 const starts=nodes.filter(n=>/TGFB1|TGF[-‐‑–—\s]?[βb][ -]?1|TGFBR/i.test(n.label));const reached=new Set(starts.map(n=>n.id));let changed=true;while(changed){changed=false;for(const e of edges)if(reached.has(e.from)&&!reached.has(e.to)){reached.add(e.to);changed=true;}}assert.ok(nodes.some(n=>nuclear(n.label)&&reached.has(n.id)));
}
export function checkTgfbCaptions(text,records=[]){assert.match(text,/人类|human|Homo sapiens/i);assert.match(text,/TGFB1|TGF[-‐‑–—\s]?[βb][ -]?1/i);assert.match(text,/SMAD4/i);assert.match(text,/入核|核转|nucle/i);assert.match(text,/简化|抽象|合并|abstract|simplif/i);
 const summary=JSON.stringify(records.find(r=>r.record?.stId==='R-HSA-170868')?.record?.summation??[]);
 if(['SMAD1','SMAD5','SMAD8'].every(name=>summary.includes(name))){
  const explicitComparison=/(?:标题|参与物)[^\n]{0,70}SMAD2\s*[/,、]\s*3[^\n]{0,80}(?:但|却|而|however)[^\n]{0,60}(?:摘要|summar)[^\n]{0,100}SMAD1\s*[/,、]\s*5\s*[/,、]\s*8/i.test(text);
  assert.ok(/冲突|不一致|不符|conflict|inconsisten/i.test(text)||explicitComparison,'observed source ambiguity remains visibly reviewable');assert.match(text,/摘要|summar|summation/i);
 }
}
export const tgfbScenario={
 name:'tgfb-smad',targetName:'TGFBR',
 initialRequest:'我想给研究生画 TGF-β 受体信号到 SMAD 入核的科研示意图，但还没确定物种、配体亚型和具体终点。先轻量检索 Reactome 候选来源，不默认人类或配体，把图意和最多三个必须澄清的问题保存到计划，保留空答案；本轮不画图、不生成候选、不选绘图素材。',
 confirmation:'确认：物种为人类，成熟活性配体为 TGFB1（TGF-β1），受体与SMAD使用野生型。范围从配体结合TGFBR2并与TGFBR1形成活化受体复合物开始，经SMAD2/SMAD3磷酸化、与SMAD4形成复合物，到该复合物进入细胞核为止，不展开下游转录、BMP/Activin/Nodal、潜伏配体活化、肿瘤突变、抑制反馈或药物。至多八个生物组件，可合并中间态，但须保留实际催化归属、胞外/膜/早期内体/胞质/核的关键区室差别并说明简化，不需要残基编号。自行发现和读取反应级来源，核对参与物、催化者与上下文，来源标题、摘要、参与物不一致时保留冲突限定，不凭一句摘要套用别的SMAD分支。每条关系保留实际读取的URL，不能用一个总览代替所有箭头。保留并回答原问题，保存就绪计划供我确认；本轮仍不画图、不添加素材。',
 captionPrompt:'添加可编辑简洁范围说明，保留人类野生型TGFB1经SMAD2/3–SMAD4入核的终点、区室过程简化以及已读来源冲突或关键不确定性，不把限定只藏在元数据。',
 checkClarification:checkTgfbClarification,checkPlan:checkTgfbPlan,checkCaptions:checkTgfbCaptions,
 scope:'fresh same-session human wild-type TGFB1-SMAD2/3-SMAD4 nuclear-entry source-to-sketch; bounded reaction and attribution checks; not complete materialization or general scientific validation',
};
