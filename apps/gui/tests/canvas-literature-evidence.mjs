import {checkAbstractCoverage} from './helpers/canvas-live-evidence.mjs';
import assert from 'node:assert/strict';
import {readFileSync} from 'node:fs';
import {literatureAbstractRecords,pubmedUrl} from './helpers/canvas-literature-evidence.mjs';
const call=(id,name,input)=>({type:'tool_use',id,name,input});const result=(id,content)=>({type:'tool_result',tool_use_id:id,content:JSON.stringify(content)});
const blocks=[call('s','literature_search',{}),result('s',{session_id:'session',candidates:[{sources:['pubmed'],title:'Paper title',identifiers:{pmid:'123',doi:'10/example'},import_candidate_id:'candidate',import_fingerprint:'fingerprint'}]}),call('i','literature_import',{session_id:'session',candidates:[{candidate_id:'candidate',expected_fingerprint:'fingerprint'}]}),result('i',{outcomes:[{article_id:'article',candidate_id:'candidate',status:'imported'}]}),call('r','bib_get_article',{article_ids:['article']}),result('r',{results:[{article_id:'article',found:true,pmid:'123',doi:'10/example',title:'Paper title',abstract:'Actual retrieved abstract.',has_fulltext:true}]})];
const wrap=items=>[{content:items}];
assert.equal(literatureAbstractRecords(wrap(blocks)).length,1);assert.equal(literatureAbstractRecords(wrap(blocks))[0].coverage,'abstract_only');
assert.equal(literatureAbstractRecords(wrap(blocks.slice(0,4))).length,0,'import and search do not establish a read');
for(const mutate of [b=>b[1].is_error=true,b=>b[3].is_error=true,b=>b[5].is_error=true,b=>b[2].input.candidates[0].expected_fingerprint='wrong',b=>b[2].input.session_id='other',b=>b[4].input.article_ids=['other'],b=>{const v=JSON.parse(b[5].content);v.results[0].pmid='456';b[5].content=JSON.stringify(v);},b=>{const v=JSON.parse(b[5].content);v.results[0].abstract='';b[5].content=JSON.stringify(v);},b=>{const v=JSON.parse(b[1].content);v.candidates[0].sources=['user'];b[1].content=JSON.stringify(v);}]){const invalid=structuredClone(blocks);mutate(invalid);assert.deepEqual(literatureAbstractRecords(wrap(invalid)),[]);}
assert.equal(pubmedUrl('http://www.ncbi.nlm.nih.gov/pubmed/123'),'https://pubmed.ncbi.nlm.nih.gov/123');assert.equal(pubmedUrl('https://pubmed.ncbi.nlm.nih.gov/123/'),'https://pubmed.ncbi.nlm.nih.gov/123');assert.equal(pubmedUrl('https://evil.example/pubmed/123'),null);assert.equal(pubmedUrl('https://user:secret@pubmed.ncbi.nlm.nih.gov/123'),null);
if(process.env.CANVAS_LITERATURE_REPLAY){const records=literatureAbstractRecords(JSON.parse(readFileSync(process.env.CANVAS_LITERATURE_REPLAY,'utf8')));assert.equal(records.length,6);assert.ok(records.every(r=>r.coverage==='abstract_only'));console.log('actual v17: six provider-discovered, fingerprint-imported and independently read abstracts; no fulltext claim');}
const paper=literatureAbstractRecords(wrap(blocks))[0];
assert.throws(()=>checkAbstractCoverage([paper.source_url],[paper]));
assert.throws(()=>checkAbstractCoverage(['abstract; read full text '+paper.source_url],[paper]));
checkAbstractCoverage(['read at abstract level; full text not retrieved '+paper.source_url],[paper]);
console.log('literature evidence: actual read chain and negative provenance/error/missing-content cases passed');

const s2=structuredClone(blocks),s2id='a'.repeat(40);
const discovery=JSON.parse(s2[1].content);discovery.candidates[0].sources=['semantic_scholar'];discovery.candidates[0].identifiers.s2=s2id;s2[1].content=JSON.stringify(discovery);
const read=JSON.parse(s2[5].content);read.results[0].identifiers=[{kind:'s2',value:s2id}];s2[5].content=JSON.stringify(read);
const s2record=literatureAbstractRecords(wrap(s2))[0];assert.equal(s2record.provider,'semantic_scholar');assert.equal(s2record.source_url,'https://pubmed.ncbi.nlm.nih.gov/123');assert.equal(s2record.coverage,'abstract_only');
for(const mutate of [b=>{const r=JSON.parse(b[1].content);delete r.candidates[0].identifiers.s2;b[1].content=JSON.stringify(r);},b=>{const r=JSON.parse(b[5].content);r.results[0].identifiers[0].value='b'.repeat(40);b[5].content=JSON.stringify(r);},b=>{const r=JSON.parse(b[5].content);delete r.results[0].identifiers;b[5].content=JSON.stringify(r);},b=>{const r=JSON.parse(b[5].content);r.results[0].doi='10/other';b[5].content=JSON.stringify(r);},b=>{const r=JSON.parse(b[5].content);r.results[0].title='Unrelated title';b[5].content=JSON.stringify(r);}]){const invalid=structuredClone(s2);mutate(invalid);assert.deepEqual(literatureAbstractRecords(wrap(invalid)),[]);}
console.log('Semantic Scholar discovery also requires matching provider ID, PMID, title, DOI, fingerprinted import and actual abstract read; availability never establishes fulltext coverage');

for(const limitation of ['仅摘要已读，未获全文','仅摘要已读，未获全文；扫描件文本抽取失败，正文未读','已阅读摘要，全文未读'])checkAbstractCoverage([limitation+' '+paper.source_url],[paper]);
for(const overclaim of ['仅摘要，但已阅读全文','摘要来源，已经读取了该论文的全文','abstract; 已读过全文'])assert.throws(()=>checkAbstractCoverage([overclaim+' '+paper.source_url],[paper]));
