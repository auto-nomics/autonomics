import {captionGuidance} from "../../src/features/canvas/canvas-composition.ts";
import {checkPortableSceneExport} from './canvas-live-source-review.mjs';
import {checkCandidateGeometryReviews} from "./canvas-scene-geometry-evidence.mjs";
import {checkCompactComposition} from './canvas-compact-composition.mjs';
import {literatureAbstractRecords,pubmedUrl} from './canvas-literature-evidence.mjs';
// Unscripted configured Agent: discover sources, clarify missing scope, retain
// answers and uncertainty, then draw the confirmed plan. Oracle IDs stay out of prompts.
import assert from 'node:assert/strict';
import {readFile,writeFile} from 'node:fs/promises';
import {join} from 'node:path';
import {currentCandidateLeaf} from './canvas-live-scientific.mjs';
const normalize=s=>String(s??'').replace(/\s/g,'');
const concept=s=>String(s??'').normalize('NFKC').toLowerCase();
const speciesQuestion=q=>/物种|species|organism|人类|小鼠/i.test(q.question);
const subtypeQuestion=q=>/亚型|subtype|干扰素.{0,12}(?:类型|哪种|哪类|型)|(?:哪种|哪类|何种).{0,12}干扰素|IFN.{0,12}(?:哪种|哪类|哪一|类型)|(?:which|what).{0,20}(?:IFN|interferon)|interferon.{0,12}(?:type|family)/i.test(q.question)||(
 /干扰素|IFN|interferon/i.test(q.question)&&/还是|或|哪|选|\b(?:or|versus|vs)\b/i.test(q.question)
 &&/(?:^|[^A-Za-z])(?:I|Ⅰ|1|一)\s*型|type\s*(?:I|Ⅰ|1)(?![A-Za-z0-9])/i.test(q.question)
 &&/(?:^|[^A-Za-z])(?:II|Ⅱ|2|二)\s*型|type\s*(?:II|Ⅱ|2)(?![A-Za-z0-9])/i.test(q.question));
const uncertainty=s=>/(JAK|激酶)/i.test(s)&&/可能|推测|不确定|未确定|未明确|不.{0,12}(确定|归因|指定|断言|区分)|probab|uncertain|unresolved|without.{0,25}(assign|specif|definit)|not.{0,25}(certain|definit|assign|specific)/i.test(s);
function evidenceReads(history){
 const calls=new Map(history.flatMap(m=>m.content).filter(b=>b.type==='tool_use'&&b.name==='figure_evidence').map(b=>[b.id,b.input]));
 const reads=[];
 for(const block of history.flatMap(m=>m.content))if(block.type==='tool_result'&&!block.is_error){
  const input=calls.get(block.tool_use_id);if(!['record','record_batch'].includes(input?.action))continue;
  let value;try{value=typeof block.content==='string'?JSON.parse(block.content):block.content;}catch{continue;}
  value=value?.content??value;
  const read=input.action==='record'?[value]:(Array.isArray(value?.results)?value.results.filter(item=>item.status==='ok'&&Array.isArray(input.ids)&&input.ids.includes(item.requested_id)):[]);
  reads.push({tool_use_id:block.tool_use_id,input,batch_counts:{requested:value?.requested_count,successful:value?.success_count,failed_or_deferred:value?.failure_count},records:read.filter(record=>record?.source_url&&record?.record)});
 }
 return reads;
}
export function sourceRecords(history){return [...evidenceReads(history).flatMap(read=>read.records),...literatureAbstractRecords(history)];}
export function successfulRecordBatches(history){return evidenceReads(history).filter(read=>read.input.action==='record_batch'&&new Set(read.records.map(record=>record.source_url)).size>=2).map(read=>({tool_use_id:read.tool_use_id,requested_ids:read.input.ids,read_ids:read.records.map(record=>record.requested_id),source_urls:read.records.map(record=>record.source_url)}));}
export function recordBatchSummary(history){
 const reads=evidenceReads(history).filter(read=>read.input.action==='record_batch');
 const totals={returned_batches:reads.length,reported_requested_items:0,reported_successful_items:0,reported_failed_or_deferred_items:0,incomplete_count_metadata_batches:0};
 for(const read of reads){const c=read.batch_counts;if(![c.requested,c.successful,c.failed_or_deferred].every(n=>Number.isSafeInteger(n)&&n>=0)){totals.incomplete_count_metadata_batches++;continue;}totals.reported_requested_items+=c.requested;totals.reported_successful_items+=c.successful;totals.reported_failed_or_deferred_items+=c.failed_or_deferred;}
 return totals;
}


export function evidenceUrls(evidence,records){
 const clean=url=>{const pubmed=pubmedUrl(url);if(pubmed)return pubmed;try{const value=new URL(url);if(value.hostname==='www.reactome.org')value.hostname='reactome.org';if(value.hostname==='reactome.org')value.protocol='https:';value.hash='';return value.toString().replace(/\/$/,'');}catch{return url;}};
 const aliases=new Map(records.flatMap(record=>[record.source_url,record.retrieval_url].filter(Boolean).map(url=>[clean(url),clean(record.source_url)])));
 return (evidence??[]).flatMap(entry=>[...String(entry).replace(/([;,；，、。])(?=https?:\/\/)/g,'$1 ').matchAll(/https?:\/\/[^\s<>"'()（）；，、。]+/g)].map(match=>clean(match[0].replace(/[.,;:，。；：、]+$/u,'')))).map(url=>aliases.get(url)??url);
}
export function unreadEvidenceUrls(planning,records) {
 const read=new Set(records.map(record=>record.source_url));
 return [...new Set(planning.document.edges.flatMap(edge=>evidenceUrls(edge.evidence,records)).filter(url=>!read.has(url)))];
}
export function checkAbstractCoverage(evidence,records){
 for(const entry of evidence??[])for(const url of evidenceUrls([entry],records)){
  const source=records.find(record=>record.source_url===url);
  if(source?.coverage==='abstract_only'){
   assert.match(String(entry),/abstract|摘要/i,'abstract-only reads require an explicit citation-level limitation');
   assert.doesNotMatch(String(entry),/(?:read|retrieved|verified)\s+(?:the\s+)?full[- ]?text|已(?:经)?(?:读(?:取)?|阅读)(?:了|过)?(?:该(?:论文|文章)的?)?全文/i,'abstract availability does not establish a fulltext read');
  }
 }
}
export function checkEvidencePlan(plan,first,records){
 const required=first.document.questions.filter(q=>q.blocking!==false),scopeIds=new Set([required.find(speciesQuestion)?.id,required.find(subtypeQuestion)?.id].filter(Boolean));
 assert.equal(plan.status,'ready');assert.ok(plan.version>first.version);assert.ok(plan.document.nodes.length>=3&&plan.document.nodes.length<=6);assert.ok(plan.document.edges.length>=2);
 for(const q of first.document.questions){const retained=plan.document.questions.find(v=>v.id===q.id);assert.ok(retained,`clarification ${q.id} remains auditable`);if(retained.blocking!==false||scopeIds.has(q.id))assert.ok(retained.answer.trim(),`required clarification ${q.id} is answered`);if(scopeIds.has(q.id))assert.notEqual(retained.blocking,false,'scientific scope remains a required clarification');}
 const gafSource=records.find(record=>record.record.stId==='R-HSA-873927'&&/STAT1/i.test(JSON.stringify(record.record))&&/GAF/i.test(JSON.stringify(record.record))&&/dimer|二聚/i.test(JSON.stringify(record.record)));
 const gafDefined=!!gafSource&&plan.document.nodes.some(node=>/stat1/i.test(node.label)&&/二聚|dimer/i.test(node.label)&&/\bgaf\b/i.test(node.label)&&evidenceUrls(node.evidence,records).includes(gafSource.source_url));
 const stat1Label=s=>/stat1/i.test(s)||(gafDefined&&/\bgaf\b/i.test(s));
 const labels=plan.document.nodes.map(n=>concept(n.label)),joined=labels.join(' ');assert.ok(/ifn[\s-]*(γ|gamma)|ifng|干扰素.*γ|伽马/.test(joined));assert.ok(/ifngr|干扰素.*受体/.test(joined));assert.ok(!/ifnar|stat2|irf9|isgf3/.test(joined),'do not substitute the type-I branch');assert.ok(/stat1/.test(joined));assert.ok(labels.some(s=>/stat1/.test(s)&&/二聚|dimer/.test(s)));assert.ok(labels.some(s=>stat1Label(s)&&/核|nucle/.test(s)));
 const scientificText=[plan.document.message,...plan.document.assumptions,...plan.document.nodes.map(n=>n.label),...plan.document.edges.map(e=>e.label??''),...plan.document.questions.map(q=>q.answer)].join('\n');assert.ok(/JAK1/i.test(scientificText)&&/JAK2/i.test(scientificText)||/JAK\s*1\s*[/／,、–-]\s*2/i.test(scientificText),'both receptor-associated kinases are retained');assert.ok(/人类|human|Homo sapiens/i.test(scientificText));assert.ok([plan.document.message,...plan.document.assumptions,...plan.document.edges.map(e=>e.label??'')].some(uncertainty),'retain uncertainty about individual JAK responsibility');
 const urls=new Set(records.map(r=>r.source_url));for(const edge of plan.document.edges){checkAbstractCoverage(edge.evidence,records);assert.ok(edge.evidence?.length,'each relationship records read sources');const cited=evidenceUrls(edge.evidence,records);assert.ok(cited.length,'references include a retrievable source URL');for(const url of cited)assert.ok(urls.has(url),`edge citation was actually retrieved: ${url}`);}
 // Independent canonical event oracle, never included in the generation prompt.
 const essential=['R-HSA-873922','R-HSA-873917'];for(const id of essential){const r=records.find(r=>r.record.stId===id);assert.ok(r,`read reaction-level source ${id}, not only a broad pathway`);assert.ok(plan.document.edges.some(e=>evidenceUrls(e.evidence,records).includes(r.source_url)),`link the relevant reaction record ${id}`);}
 const nodeIndex=new Map(plan.document.nodes.map(n=>[n.id,n]));const nuclearLabel=s=>stat1Label(s)&&/核|nucle/i.test(s),dimerLabel=s=>/stat1/i.test(s)&&/二聚|dimer/i.test(s),phosphoLabel=s=>/stat1/i.test(s)&&/磷酸|phosph|(?:Y|Tyr)701|p[- _]?stat1/i.test(s)&&!/unphosph|未磷酸|非磷酸/i.test(s);
 for(const edge of plan.document.edges){const from=nodeIndex.get(edge.from)?.label??'',to=nodeIndex.get(edge.to)?.label??'';if(nuclearLabel(to)&&!nuclearLabel(from))assert.ok(evidenceUrls(edge.evidence,records).includes('https://reactome.org/content/detail/R-HSA-873917'),'nuclear transport source belongs to its actual transition');if(/STAT1\s*(?:磷酸化|phosphorylation)|(?:磷酸化|phosphorylation of)\s*STAT1/i.test(edge.label??'')||phosphoLabel(to)&&!phosphoLabel(from))assert.ok(evidenceUrls(edge.evidence,records).includes('https://reactome.org/content/detail/R-HSA-873922'),'phosphorylation source belongs to its actual transition');if(dimerLabel(to)&&!dimerLabel(from))assert.ok(evidenceUrls(edge.evidence,records).some(url=>/R-HSA-(873922|873927)$/.test(url)),'dimerization has a read source that describes it');}
 const index=new Map(plan.document.nodes.map(n=>[n.id,n])),nuclear=plan.document.nodes.filter(n=>stat1Label(n.label)&&/核|nucle/i.test(n.label));assert.ok(nuclear.length);for(const node of nuclear)if(!/stat1/i.test(node.label))assert.ok(evidenceUrls(node.evidence,records).includes('https://reactome.org/content/detail/R-HSA-873917'),'a nuclear GAF alias retains the actually read transport source on its node');
 const incoming=new Set(plan.document.edges.map(edge=>edge.to));const upstream=plan.document.nodes.filter(n=>!incoming.has(n.id)&&/ifng|ifn[\s-]*(γ|gamma)|ifngr|干扰素|伽马/i.test(n.label));const seen=new Set(upstream.map(n=>n.id));let changed=true;while(changed){changed=false;for(const edge of plan.document.edges){assert.ok(index.has(edge.from)&&index.has(edge.to));if(seen.has(edge.from)&&!seen.has(edge.to)){seen.add(edge.to);changed=true;}}}assert.ok(nuclear.some(n=>seen.has(n.id)),'declared signal has a directed path to nuclear STAT1');
}
export function checkEvidenceExport(manifest, document, records) {
  const sources = manifest.relation_sources;
  const arrows = document.elements.filter(element => element.type === 'arrow' && !element.isDeleted);
  assert.equal(sources.verification, 'not_verified');
  assert.equal(sources.total, arrows.length);
  assert.equal(sources.relations.length, arrows.length);
  for (const relation of sources.relations) {
    const arrow = arrows.find(element => element.id === relation.element_id);
    assert.ok(arrow, 'exported evidence belongs to a real relationship');
    const expected = arrow.customData.evidence;
    checkAbstractCoverage(expected,records);
    assert.equal(relation.reference_count, expected.length);
    assert.equal(relation.references_truncated, false);
    assert.deepEqual(relation.references, expected.map(text => ({kind:'text',text:text.slice(0,2000),truncated:text.length>2000})), 'export preserves each supplied source or abstraction note on its actual relationship');
    const urls = relation.references.flatMap(ref => evidenceUrls([ref.text], records));
    assert.ok(urls.length, 'each exported relationship retains actual source URLs');
    assert.ok(urls.every(url => records.some(record => record.source_url === url)), 'exported URLs were actually read');
  }
}
export function checkEvidenceClarification(first, calls) {
  assert.equal(first.status, 'needs_clarification');
  const required = first.document.questions.filter(q => q.blocking !== false);
  const species = required.find(speciesQuestion), subtype = required.find(subtypeQuestion);
  assert.ok(species && subtype, 'organism and subtype remain explicit scientific clarifications');
  assert.ok(required.length <= 3, 'start with at most three required clarifications');
  assert.equal(species.answer.trim(), '');
  assert.equal(subtype.answer.trim(), '');
  const searches = calls.filter(call => !call.is_error && call.name === 'figure_evidence' && call.input.action === 'search');
  assert.ok(searches.length, 'source discovery is actual tool activity');
  assert.equal(searches[0].input.species ?? null, null, 'initial discovery must not assume organism');
  // Later explicit species comparisons do not resolve the still-unanswered scope.
}
export function checkSketchPlanLifecycle(before,after,proposals) {
 const content=value=>Object.fromEntries(Object.entries(value).filter(([key])=>!['status','proposal_id','updated_at'].includes(key)));
 assert.deepEqual(content(after),content(before),'the confirmed scientific plan content and version stay unchanged');
 assert.equal(before.status,'ready');assert.equal(after.status,'proposed');
 assert.ok(Number.isFinite(Date.parse(after.updated_at))&&Date.parse(after.updated_at)>=Date.parse(before.updated_at));
 const root=proposals.find(proposal=>proposal.id===after.proposal_id);
 assert.ok(root&&!root.parent_proposal_id,'the lifecycle links the actual root sketch proposal');
 assert.equal(root.base_revision,before.based_on_revision);
 const source=root.document.elements.find(element=>element.customData?.planningSource)?.customData.planningSource;
 assert.ok(source,'the sketch retains the exact reviewed planning snapshot');
 assert.equal(source.version,before.version);assert.equal(source.based_on_revision,before.based_on_revision);
 assert.deepEqual(source.document,before.document);
}
export async function runLiveEvidencePlanning({page,call,path,handoff,send,persist,directory,turns,until,scenario}){
 const materialPreference=process.env.VISUALIZATION_LIVE_SECOND_MATERIAL==='1'?' 本图在确认范围后希望完成多组件素材化，受体和确认后的下游 STAT 组件分别使用合适的目录素材。构图优先三列视觉分面、各面内部纵向，为素材标签和图注留出空间，不用空白宽文字框凑比例；本轮仍只澄清，不画图或选素材。':'';
 await handoff((scenario?.initialRequest??'我准备给研究生讲 IFN 刺激后 STAT1 进入细胞核的机制，但还没确定物种和干扰素亚型。请先从 Reactome 检索相关候选来源，明确哪些信息缺失；不要默认人类或默认某个亚型。把图意、已知受众和待澄清问题保存到绘图计划，保留空答案供下一轮回答。这轮不要画图、生成候选或添加素材。')+materialPreference);
 const formal=await call('GET',path);await send(1);const first=await call('GET',`${path}/planning`);await persist('evidence-first-plan.json',first);(scenario?.checkClarification??checkEvidenceClarification)(first,turns[0].calls);assert.deepEqual(await call('GET',`${path}/proposals`),[]);assert.deepEqual(await call('GET',path),formal);
 const batchInstruction=process.env.VISUALIZATION_LIVE_REQUIRE_BATCH_EVIDENCE==='1'?'多个相互独立且已发现的反应记录请至少一次使用 figure_evidence record_batch 同批读取两到四条，ids 使用原生数组；只将逐条成功返回的实际来源纳入依据，失败项按需单独补读。':'';
 await send(2,(scenario?.confirmation??'确认：物种是人类，亚型是 IFN-γ（IFNG），从它结合 IFNGR 受体复合物开始，展示受体相关 JAK、STAT1 磷酸化、同源二聚化到进入细胞核为止；不展开核内转录、药物、反馈抑制或 MAPK 支路。至多六个生物组件，可以合并复合物和中间态，排版由你选择。请自己检索并读取 Reactome 的相关反应级记录，逐条保留来源 URL，把关键抽象和来源中的不确定性写进计划；不能只引用通路总览来证明每条箭头。保留上一轮问题的 ID 并填写这轮答案，保存可供我确认的就绪计划。本轮仍不画图或生成候选；如来源不足就明确说明，不猜测。')+batchInstruction);
 let planning=await call('GET',`${path}/planning`),records=sourceRecords(JSON.parse(await readFile(join(directory,'session-history.json'),'utf8'))),sourceReviewTurns=0;
 const unread=unreadEvidenceUrls(planning,records);
 if(process.env.VISUALIZATION_LIVE_REVIEW_UNREAD_SOURCES==='1'&&unread.length){
  await persist('evidence-plan-before-source-review.json',planning);await persist('evidence-records-before-source-review.json',records);
  const request=`计划审阅：下列独立引用 URL 尚未在本次会话中实际读取：\n${unread.join('\n')}\nReactome 实体须用 figure_evidence record 核对；文献须实际检索、导入并读取，检索摘要片段、导入回执或下载任务不算已读。只读到摘要时须在该引用处明确注明摘要范围，不能声称读到全文。反应记录中的参与者或参考文献摘要不等于已读取该实体或论文。请补读这些来源；若无法读取或不支持所写关系，改用已经实际核对、且支持同一转变的反应来源，并明确相关抽象与不确定性。不要捏造证据，不为补引用新增分子或支路；保留问题 ID、已有答案和当前科学范围。本轮仍不画图、不生成候选、不添加素材，更新并保存供我确认的就绪计划。`;
  await persist('evidence-source-review-feedback.json',{unread_urls:unread,request});await send(turns.length+1,request);sourceReviewTurns++;
  planning=await call('GET',`${path}/planning`);records=sourceRecords(JSON.parse(await readFile(join(directory,'session-history.json'),'utf8')));
 }
 let scientificReviewTurns=0;
 const scientificFeedback=scenario?.reviewPlan?.(planning,records);
 if(scientificFeedback){
  await persist('evidence-plan-before-scientific-review.json',planning);
  await persist('evidence-scientific-review-feedback.json',{request:scientificFeedback,scope:'explicit bounded scientific review; no scene, candidate or source-answer injection'});
  await send(turns.length+1,scientificFeedback);scientificReviewTurns++;
  planning=await call('GET',`${path}/planning`);records=sourceRecords(JSON.parse(await readFile(join(directory,'session-history.json'),'utf8')));
 }
 const batches=successfulRecordBatches(JSON.parse(await readFile(join(directory,'session-history.json'),'utf8')));const batchSummary=recordBatchSummary(JSON.parse(await readFile(join(directory,'session-history.json'),'utf8')));await persist('evidence-batch-summary.json',batchSummary);await persist('evidence-batch-reads.json',batches);if(process.env.VISUALIZATION_LIVE_REQUIRE_BATCH_EVIDENCE==='1')assert.ok(batches.length,'actual batch must deliver at least two distinct successfully read sources');
 await persist('evidence-records.json',records);await persist('evidence-ready-plan.json',planning);(scenario?.checkPlan??checkEvidencePlan)(planning,first,records);assert.deepEqual((await call('GET',`${path}/planning/revisions/${first.version}`)).document,first.document);assert.deepEqual(await call('GET',path),formal);assert.deepEqual(await call('GET',`${path}/proposals`),[]);
 const sketchIndex=turns.length;
 const conciseCaption=process.env.VISUALIZATION_LIVE_CONCISE_CAPTION==='1';
 const captionInstruction=conciseCaption?captionGuidance('concise')+'本次采用简洁图注：图内只保留范围、终点和关键不确定性的简短说明，正文使用至少 16 的字号，不逐条列出完整 http/https 地址；完整来源 URL 必须原样留在已确认节点/关系 evidence 中并能随来源清单导出，不能删引用来压缩画面。':'';
 if(process.env.VISUALIZATION_LIVE_LAYOUT_PREFERENCE==='compact'){assert.equal(planning.layout_review?.status,'available','compact model evaluation must receive actual compiler diagnostics');assert.ok(planning.layout_review.bounds.width>0&&planning.layout_review.bounds.height>0);}
 const captionPrompt=scenario?.captionPrompt??'图下添加可编辑范围说明，明确人类 IFN-γ 分支和入核终点，并保留 JAK 激酶归属的不确定性；不能把假设只藏在元数据里。';
 const textInstruction=process.env.VISUALIZATION_LIVE_REQUIRE_SCALAR_TEXT==='1'?'添加说明必须使用 visualization_add_text，x/y/max_width/font_size 均为顶层 JSON 数值，不传 operations。':'添加说明可使用 visualization_add_text 的顶层数值入口。';
 await page.getByRole('tab',{name:'Live figure',exact:true}).click();await page.getByRole('button',{name:'绘图计划',exact:true}).click();const planDialog=page.getByRole('dialog',{name:'科研绘图计划',exact:true});await until(async()=>await planDialog.getByRole('textbox',{name:'核心图意',exact:true}).inputValue()===planning.document.message,'GUI shows the saved brief');assert.equal(await planDialog.getByRole('textbox',{name:'计划受众',exact:true}).inputValue(),planning.document.audience);for(const [i,q] of planning.document.questions.entries())assert.equal(await planDialog.getByRole('textbox',{name:`问题答案 ${i+1}`,exact:true}).inputValue(),q.answer);await page.screenshot({path:join(directory,'evidence-plan-review.png'),animations:'disabled'});await planDialog.getByRole('button',{name:'关闭计划',exact:true}).click();
 const agentPanel=page.getByRole('button',{name:'Agent 绘图',exact:true});if(await agentPanel.getAttribute('aria-expanded')!=='true')await agentPanel.click();await page.getByRole('textbox',{name:'绘图需求'}).fill(`我确认当前已保存的就绪计划。请按该版本计划生成可编辑草图，保留全部节点、关系、来源 URL 和科学抽象。${captionInstruction}${captionPrompt}${textInstruction} 当前计划已确认，本轮不要 save_plan；首次 sketch_plan 后只在返回的候选上顺序修订，每次使用返回的新 proposal_id/document_sha256，不再次 sketch_plan，不用 propose 另起根候选，也不创建同父备选。失败时读取当前候选再恢复，不重建科学图。按已读记录的 schemaClass 区分分子/复合物实体与反应/转运事件，不得把实体编号称为反应编号；说明文字中的引用也只能支持实际读到的内容。科学标签和关系标签的文字必须完整保留，不要为修复断行而缩写、删掉分子名或改写标签；需要改善显示时仅调整换行或字号，并保留 originalText 的完整内容。不要增加计划外分子、支路或引文，本轮不检索或插入绘图素材。读取最终候选 PNG 检查标签和关系后留待我审阅，不采用。`);await page.getByRole('button',{name:'交给 Agent',exact:true}).click();await page.getByPlaceholder('输入消息…').waitFor();await send(sketchIndex+1);
 const afterSketchPlan=await call('GET',`${path}/planning`);await persist('evidence-after-sketch-plan.json',afterSketchPlan);
 if(process.env.VISUALIZATION_LIVE_REQUIRE_SCALAR_TEXT==='1')assert.ok(turns[sketchIndex].calls.some(c=>!c.is_error&&c.name==='visualization_add_text'),'continuous evaluation requires a successful scalar text call');
 const all=await call('GET',`${path}/proposals`);await persist('evidence-all-candidates.json',all);checkSketchPlanLifecycle(planning,afterSketchPlan,all);const candidate=currentCandidateLeaf(all,formal.metadata.revision);await persist('evidence-candidate.json',candidate);if(process.env.VISUALIZATION_LIVE_REQUIRE_GEOMETRY_REVIEW==='1'){const reviews=checkCandidateGeometryReviews(JSON.parse(await readFile(join(directory,'session-history.json'),'utf8')),candidate,{projectId:formal.metadata.project_id});await persist('evidence-scene-geometry-review.json',reviews);}const doc=candidate.document,anchors=doc.elements.filter(e=>e.customData?.scientificId),arrows=doc.elements.filter(e=>e.type==='arrow'&&!e.isDeleted);assert.equal(anchors.length,planning.document.nodes.length);assert.equal(arrows.length,planning.document.edges.length);assert.equal(doc.elements.filter(e=>e.type==='image'&&!e.isDeleted).length,0);
 for(const node of planning.document.nodes){const anchor=anchors.find(e=>e.customData.scientificId===node.id);assert.ok(anchor);const label=doc.elements.find(e=>e.type==='text'&&e.containerId===anchor.id);assert.ok(label);assert.equal(normalize(label.originalText??label.text),normalize(node.label));assert.equal(normalize(label.text),normalize(node.label),"visible node label preserves confirmed scientific text");assert.deepEqual(anchor.customData.evidence,node.evidence??[]);}
 for(const edge of planning.document.edges){const arrow=arrows.find(a=>a.customData?.from===edge.from&&a.customData?.to===edge.to);assert.ok(arrow);assert.equal(doc.elements.find(e=>e.id===arrow.startBinding.elementId).customData.scientificId,edge.from);assert.equal(doc.elements.find(e=>e.id===arrow.endBinding.elementId).customData.scientificId,edge.to);assert.deepEqual(arrow.customData.evidence,edge.evidence);if(edge.label){const label=doc.elements.find(e=>e.type==='text'&&e.containerId===arrow.id);assert.ok(label);assert.equal(normalize(label.originalText??label.text),normalize(edge.label));assert.equal(normalize(label.text),normalize(edge.label),"visible relationship label preserves confirmed scientific text");}}
 if(process.env.VISUALIZATION_LIVE_LAYOUT_PREFERENCE==='compact')checkCompactComposition(planning.document,doc);
 if(conciseCaption){const notes=doc.elements.filter(e=>e.type==='text'&&!e.isDeleted&&!e.containerId);assert.ok(notes.length);assert.ok(notes.every(e=>e.fontSize>=16),'concise captions keep requested readable native font size');assert.ok(notes.every(e=>!/https?:\/\//i.test(e.originalText??e.text)),'full URLs remain in evidence rather than the requested concise visible caption');}
 const standaloneText=doc.elements.filter(e=>e.type==='text'&&!e.isDeleted&&!e.containerId);
 await persist('evidence-text-layout-audit.json',{proposal_id:candidate.id,document_sha256:candidate.document_sha256,scope:'standalone text metrics and Latin token presence; not caption classification, visible-glyph overlap or scientific validation',standalone_text_count:standaloneText.length,standalone_text_characters:standaloneText.reduce((sum,e)=>sum+Array.from(e.originalText??e.text??'').length,0),texts:standaloneText.map(e=>({id:e.id,characters:Array.from(e.originalText??e.text??'').length,rendered_lines:String(e.text??'').split('\n').length,font_size:e.fontSize,width:e.width,height:e.height})),text_tokens:doc.elements.filter(e=>e.type==='text'&&!e.isDeleted).map(e=>({id:e.id,original_latin_tokens_absent_as_contiguous_text:[...new Set(String(e.originalText??e.text??'').match(/[A-Za-z][A-Za-z0-9-]{2,}/g)??[])].filter(token=>!String(e.text??'').includes(token))})).filter(e=>e.original_latin_tokens_absent_as_contiguous_text.length)});
 const captions=doc.elements.filter(e=>e.type==='text'&&!e.containerId).map(e=>e.originalText??e.text).join('\n');if(scenario?.checkCaptions)scenario.checkCaptions(captions,records);else{assert.ok(/人类|human|Homo sapiens/i.test(captions));assert.ok(/IFN.*γ|IFNG|IFN.*gamma|伽马/i.test(captions));assert.ok(/入核|核转|nucle/i.test(captions));assert.ok(uncertainty(captions),'uncertainty remains visible');}assert.deepEqual(await call('GET',path),formal);assert.ok(turns[sketchIndex].calls.some(c=>!c.is_error&&c.name==='visualization'&&c.input.action==='read_proposal_preview'&&c.input.proposal_id===candidate.id));
 let png;await until(async()=>{png=await call('GET',`${path}/proposals/${candidate.id}/preview?expected_revision=${candidate.base_revision}`);assert.notEqual(png.render_status,'failed');return png.render_status==='ready';},'source-informed candidate PNG');assert.equal(png.document_sha256,candidate.document_sha256);await writeFile(join(directory,'evidence-candidate.png'),Buffer.from(png.dataURL.split(',')[1],'base64'));
 await page.getByRole('tab',{name:'Live figure',exact:true}).click();await page.getByRole('button',{name:'查看 Agent 提案',exact:true}).click();const review=page.getByRole('dialog',{name:'Agent 修改预览',exact:true});const matching=(await call('GET',`${path}/proposal-index?limit=20`)).items.filter(p=>p.summary===candidate.summary&&p.base_revision===candidate.base_revision&&Boolean(p.parent_proposal_id)===Boolean(candidate.parent_proposal_id));await review.getByRole('button',{name:`${candidate.summary} · v${candidate.base_revision}${candidate.parent_proposal_id?' · 修订':''}`,exact:true}).nth(matching.findIndex(p=>p.id===candidate.id)).click();await review.getByRole('region',{name:'候选画布'}).locator('.excalidraw').waitFor();await page.screenshot({path:join(directory,'evidence-figure-review.png'),animations:'disabled'});await review.getByRole('button',{name:'采用提案',exact:true}).click();await review.waitFor({state:'hidden'});const adopted=await call('GET',path);assert.equal(adopted.metadata.revision,formal.metadata.revision+1);assert.deepEqual(adopted.scene.document,doc);
 const download=page.waitForEvent('download');await page.getByRole('button',{name:'导出素材来源',exact:true}).click();const manifest=JSON.parse(await readFile(await(await download).path(),'utf8'));checkEvidenceExport(manifest,doc,records);await persist('evidence-source-manifest.json',manifest);await persist('final.json',adopted);
 const nativeDownload=page.waitForEvent('download');await page.getByRole('button',{name:'导出 源文件',exact:true}).click();const exported=JSON.parse(await readFile(await(await nativeDownload).path(),'utf8'));checkPortableSceneExport(exported,doc);await persist('evidence-source.json',exported);
 return{record_batch_summary:batchSummary,batch_evidence_required:process.env.VISUALIZATION_LIVE_REQUIRE_BATCH_EVIDENCE==='1',successful_record_batch_count:batches.length,full_scene_geometry_review_verified:process.env.VISUALIZATION_LIVE_REQUIRE_GEOMETRY_REVIEW==='1',scientific_case:scenario?.name??'ifng',compiled_layout_review:planning.layout_review??null,concise_caption_verified:conciseCaption,planning_turns:turns.length,source_review_turns:sourceReviewTurns,scientific_review_turns:scientificReviewTurns,scalar_text_required:process.env.VISUALIZATION_LIVE_REQUIRE_SCALAR_TEXT==='1',scalar_text_used:turns[sketchIndex].calls.some(c=>!c.is_error&&c.name==='visualization_add_text'),source_record_count:records.length,source_context_available_record_count:records.filter(record=>record.source_context?.pathway_context?.status==='available').length,source_context_unavailable_record_count:records.filter(record=>record.source_context?.pathway_context?.status==='unavailable').length,reactome_record_count:records.filter(record=>record.provider==='Reactome').length,literature_abstract_count:records.filter(record=>record.coverage==='abstract_only').length,plan_versions:[first.version,planning.version],scope:scenario?.scope??'one actual Agent discovers Reactome records after explicit scope clarification, retains answers and JAK uncertainty, then draws the confirmed human IFNG-to-nuclear-STAT1 abstraction; no universal scientific validity claim'};
}
