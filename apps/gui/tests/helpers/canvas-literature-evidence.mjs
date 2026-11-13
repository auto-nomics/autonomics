// Source coverage from an actual provider discovery -> fingerprinted import -> abstract read.
// Search snippets, download/import acknowledgements and fulltext availability are not reads.
const text=value=>typeof value==='string'?value.trim():'';
const title=value=>text(value).normalize('NFKC').replace(/\s+/g,' ').toLowerCase();
export function pubmedUrl(value){
 try{const u=new URL(value);if(u.username||u.password||u.search)return null;
 const match=u.hostname==='pubmed.ncbi.nlm.nih.gov'?u.pathname.match(/^\/(\d+)\/?$/):['www.ncbi.nlm.nih.gov','ncbi.nlm.nih.gov'].includes(u.hostname)?u.pathname.match(/^\/pubmed\/(\d+)\/?$/):null;
 return match&&Number(match[1])>0?`https://pubmed.ncbi.nlm.nih.gov/${match[1]}`:null;
 }catch{return null;}
}
export function literatureAbstractRecords(history){
 const calls=new Map(),candidates=new Map(),imports=new Map(),records=new Map();
 for(const block of history.flatMap(message=>message.content??[])){
  if(block.type==='tool_use'){calls.set(block.id,block);continue;}
  if(block.type!=='tool_result'||block.is_error)continue;
  const call=calls.get(block.tool_use_id);if(!call)continue;
  let result;try{result=typeof block.content==='string'?JSON.parse(block.content):block.content;result=result?.content??result;}catch{continue;}
  if(call.name==='literature_search')for(const candidate of result?.candidates??[]){
   const provider=candidate.sources?.includes('pubmed')?'pubmed':candidate.sources?.includes('semantic_scholar')&&/^[a-f0-9]{40}$/i.test(text(candidate.identifiers?.s2))?'semantic_scholar':null;
   if(!provider||!/^\d+$/.test(text(candidate.identifiers?.pmid))||!text(candidate.import_candidate_id)||!text(candidate.import_fingerprint)||!text(candidate.title))continue;
   candidates.set(`${result.session_id}:${candidate.import_candidate_id}`,{...candidate,discovery_provider:provider});
  }
  if(call.name==='literature_import')for(const outcome of result?.outcomes??[]){
   const requested=call.input?.candidates?.find(item=>item.candidate_id===outcome.candidate_id);
   const candidate=candidates.get(`${call.input?.session_id}:${outcome.candidate_id}`);
   if(candidate&&requested?.expected_fingerprint===candidate.import_fingerprint&&['imported','already_present'].includes(outcome.status)&&!outcome.error&&text(outcome.article_id))imports.set(outcome.article_id,candidate);
  }
  if(call.name==='bib_get_article')for(const article of result?.results??[]){
   const candidate=imports.get(article.article_id);
   if(!candidate||!call.input?.article_ids?.includes(article.article_id)||article.found!==true||!text(article.abstract)||title(article.title)!==title(candidate.title)||text(article.pmid)!==candidate.identifiers.pmid)continue;
   if(candidate.discovery_provider==='semantic_scholar'&&!article.identifiers?.some(id=>id.kind==='s2'&&text(id.value).toLowerCase()===text(candidate.identifiers.s2).toLowerCase()))continue;
   if(text(candidate.identifiers.doi)&&text(article.doi).toLowerCase()!==text(candidate.identifiers.doi).toLowerCase())continue;
   const source_url=`https://pubmed.ncbi.nlm.nih.gov/${article.pmid}`;
   records.set(source_url,{source_url,provider:candidate.discovery_provider,coverage:'abstract_only',verification:'not_verified',article_id:article.article_id,discovery_candidate_id:candidate.import_candidate_id,record:{schemaClass:'LiteratureAbstract',pmid:article.pmid,title:article.title,abstract:article.abstract}});
  }
 }
 return [...records.values()];
}
