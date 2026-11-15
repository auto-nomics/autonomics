import assert from 'node:assert/strict';
import { build } from 'esbuild';
import { createHash } from 'node:crypto';
const bundled = async name => {
  const result = await build({entryPoints:[`src/features/bibliography/${name}.ts`],bundle:true,platform:'node',format:'esm',write:false});
  return import(`data:text/javascript;base64,${Buffer.from(result.outputFiles[0].text).toString('base64')}`);
};
const {parseZoteroBundle,matchBundleFile,safeBundlePath}=await bundled('zotero-bundle-model');
const {migrateZoteroBundle}=await bundled('zotero-bundle-runner');
const contents=JSON.stringify({items:[
  {key:'PAPER',data:{itemType:'journalArticle',title:'Paper',collections:['CHILD'],notes:[{key:'INLINE',note:'Inline note'}],attachments:[{key:'PDF',path:'files/PAPER/article.pdf',title:'Original PDF'}]}},
  {key:'NOTE',data:{itemType:'note',parentItem:'PAPER',note:'Child note'}},
  {key:'UNSAFE',data:{itemType:'attachment',parentItem:'PAPER',path:'../../secret.txt'}},
],collections:{ROOT:{name:'Parent'},CHILD:{name:'Child',parentCollection:'ROOT',items:['PAPER']}}});
const plan=parseZoteroBundle(contents);
assert.equal(plan.records.length,1);assert.equal(plan.records[0].notes.length,2);assert.equal(plan.records[0].attachments.length,2);
assert.equal(plan.collections[1].parent,'ROOT');
for(const path of ['/etc/passwd','../secret','a/../../secret','C:\\secret','https://example.org/file','file:///tmp/x'])assert.equal(safeBundlePath(path),null);
const file=new File(['PDF fixture'],'article.pdf',{type:'application/pdf'});
Object.defineProperty(file,'webkitRelativePath',{value:'Export/files/PAPER/article.pdf'});
assert.equal(matchBundleFile('files/PAPER/article.pdf',[file]),file);
assert.equal(matchBundleFile('/etc/passwd',[file]),null);
assert.equal(matchBundleFile('other/article.pdf',[file]),null);
assert.throws(()=>matchBundleFile('files/PAPER/article.pdf',[file,file]),/Ambiguous/);
assert.throws(()=>parseZoteroBundle(JSON.stringify({items:[{title:'x'}],collections:{a:{name:'a',parentCollection:'b'},b:{name:'b',parentCollection:'a'}}})),/cycle/);
assert.throws(()=>parseZoteroBundle(JSON.stringify({items:[{key:'same',title:'x'},{key:'same',title:'y'}]})),/Duplicate source/);
const collections=[],notes=[],attachments=[],calls=[];let imported=false,failUpload=true;
const request=async(method,path,body)=>{
  calls.push({method,path,body});
  if(path==='/bib/import/commit'){const imported_ids=imported?[]:['stored'];imported=true;return{imported_ids,skipped_ids:[]};}
  if(path==='/bib/import/preview')return{records:[{index:0,duplicate_id:'stored',article:{id:'imported',title:'Paper'},warnings:['Non-author contributor roles are not mapped']}]};
  if(path==='/bib/collections'&&method==='GET')return{collections};
  if(path==='/bib/collections'&&method==='POST'){const collection={...body,id:`collection-${collections.length}`};collections.push(collection);return{collection};}
  if(path==='/bib/articles/stored'&&method==='GET')return{annotations:notes.map(note=>({...note})),attachments:[...attachments],fulltext:{attachment_key:'existing-preferred'}};
  if(path==='/bib/articles/stored/annotations'&&method==='POST'){const annotation={id:`note-${notes.length}`,content:body.content,tags:[]};notes.push(annotation);return{annotation};}
  if(path.startsWith('/bib/annotations/')&&method==='PATCH'){assert.ok(body.tags.length <= 50 && body.tags.every(tag => tag.length > 0 && tag.length <= 80), 'Match API annotation tag limits');const note=notes.find(value=>value.id===path.split('/').pop());note.tags=body.tags;return{annotation:note};}
  if(path.endsWith('/articles')&&method==='POST')return{ok:true};
  throw new Error(`Unexpected ${method} ${path}`);
};
const upload=async(id,file,options)=>{
  assert.equal(id,'stored');assert.equal(options.preferred,false);assert.equal(options.title,'Original PDF');
  if(failUpload)throw new Error('Temporary upload error');
  attachments.push({storage_hash:createHash('sha256').update(Buffer.from(await file.arrayBuffer())).digest('hex')});return{};
};
const run=()=>migrateZoteroBundle({contents,plan,files:[file],sourceLabel:'My Zotero',request,upload,noteText:value=>value,progress:()=>{}});
const failed=await run();
assert.ok(failed.warnings.includes('Paper: Non-author contributor roles are not mapped'));
assert.equal(failed.notes,2);assert.equal(failed.collections,2);assert.equal(failed.errors.length,1);assert.match(failed.errors[0],/Temporary upload/);
assert.equal(collections[1].parent_id,collections[0].id);assert.ok(failed.warnings.some(value=>value.includes('../../secret')));
assert.ok(notes.every(value=>value.tags[0].startsWith('zotero-note:')));
failUpload=false;
const retry=await run();assert.equal(retry.notes,0);assert.equal(retry.collections,0);assert.equal(retry.attachments,1);assert.equal(retry.errors.length,0);
const again=await run();assert.equal(again.attachments,0);assert.equal(notes.length,2);assert.equal(collections.length,2);
assert.ok(calls.some(call=>call.path===`/bib/collections/${collections[1].id}/articles`&&call.body.article_id==='stored'));
console.log('PASS Zotero bundle: parent mapping, hierarchy, path traversal/ambiguity rejection, note provenance, partial retry, attachment hash dedup and preferred-file preservation');
