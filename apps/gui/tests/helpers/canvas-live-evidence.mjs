import assert from 'node:assert/strict';
import {literatureAbstractRecords,pubmedUrl} from './canvas-literature-evidence.mjs';
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
export function checkAbstractCoverage(evidence,records){
 for(const entry of evidence??[])for(const url of evidenceUrls([entry],records)){
  const source=records.find(record=>record.source_url===url);
  if(source?.coverage==='abstract_only'){
   assert.match(String(entry),/abstract|摘要/i,'abstract-only reads require an explicit citation-level limitation');
   assert.doesNotMatch(String(entry),/(?:read|retrieved|verified)\s+(?:the\s+)?full[- ]?text|已(?:经)?(?:读(?:取)?|阅读)(?:了|过)?(?:该(?:论文|文章)的?)?全文/i,'abstract availability does not establish a fulltext read');
  }
 }
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
