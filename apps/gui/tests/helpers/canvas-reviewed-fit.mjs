/** Match the exact owned illustration in the mutation's immutable base candidate. */
export function reviewedFit(call,proposals,targetId){
 const input=call.input??{},base=proposals.find(p=>p.id===input.proposal_id);
 if(call.is_error||!base||!input.document_sha256||base.document_sha256!==input.document_sha256)return false;
 const elements=(base.document?.elements??[]).filter(e=>!e.isDeleted);
 const anchor=elements.find(e=>e.id===targetId);
 const image=elements.find(e=>e.id===anchor?.customData?.visualAssetElementId&&e.type==='image');
 if(!anchor||!image||typeof image.fileId!=='string'||!image.fileId||image.customData?.semanticAnchorId!==targetId)return false;
 const matches=(id,file,bounds)=>[targetId,image.id].includes(id)&&file===image.fileId&&Array.isArray(bounds)&&bounds.length===4&&bounds.every(n=>typeof n==='number'&&Number.isFinite(n))&&bounds[0]>=0&&bounds[1]>=0&&bounds[2]>0&&bounds[3]>0&&bounds[0]+bounds[2]<=1+1e-9&&bounds[1]+bounds[3]<=1+1e-9;
 if(call.name==='visualization_fit')return matches(input.element_id,input.file_id,[input.x,input.y,input.width,input.height]);
 return call.name==='visualization'&&input.action==='revise_proposal'&&input.operations?.some(op=>op.op==='fit_illustration_anchor'&&matches(op.id,op.file_id,op.normalized_bounds))===true;
}
