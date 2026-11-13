type Row=Record<string,unknown>;
const row=(v:unknown):Row=>v&&typeof v==='object'&&!Array.isArray(v)?v as Row:{};
/** Exact local SVG bytes + backend receipt; still only sampled alpha geometry. */
export async function svgPreviewFitBounds(candidate:unknown,imageId:string,receipt:unknown){
 const c=row(candidate),document=row(c.document),proof=row(receipt),elements=Array.isArray(document.elements)?document.elements.map(row):[],matches=elements.filter(e=>e.id===imageId&&e.isDeleted!==true),image=matches.length===1?matches[0]:null;
 if(!image||image.type!=='image'||image.crop!=null||(image.angle??0)!==0)throw Error('仅支持未裁剪、未旋转的 SVG 插图。');
 if(proof.proposal_id!==c.id||proof.document_sha256!==c.document_sha256||typeof c.document_sha256!=='string'||!c.document_sha256)throw Error('采样结果不属于当前候选，请重新读取。');
 const infos=Array.isArray(proof.infos)?proof.infos.map(row).filter(i=>i.element_id===imageId):[],info=infos.length===1?infos[0]:null,bounds=row(info?.svg_preview_alpha_bounds);
 if(!info||info.file_id!==image.fileId||bounds.status!=='available'||bounds.method!=='nonzero_8bit_alpha_with_one_preview_pixel_padding'||bounds.scope!=='uncropped_file_normalized_svg_preview')throw Error('当前插图没有可用的 SVG 可见范围，请使用手动范围。');
 const values=bounds.normalized_bounds;
 if(!Array.isArray(values)||values.length!==4||!values.every(v=>typeof v==='number'&&Number.isFinite(v))||values[0]<0||values[1]<0||values[2]<=0||values[3]<=0||values[0]+values[2]>1.000001||values[1]+values[3]>1.000001)throw Error('SVG 采样范围无效。');
 const file=row(row(document.files)[String(image.fileId)]),url=file.dataURL,prefix='data:image/svg+xml;base64,';
 if(file.mimeType!=='image/svg+xml'||typeof url!=='string'||!url.startsWith(prefix)||url.length>12*1024*1024)throw Error('当前 SVG 文件不可验证。');
 const bytes=Uint8Array.from(atob(url.slice(prefix.length)),v=>v.charCodeAt(0)),hash=Array.from(new Uint8Array(await crypto.subtle.digest('SHA-256',bytes))).map(b=>b.toString(16).padStart(2,'0')).join('');
 if(info.sha256!==hash||bounds.file_sha256!==hash)throw Error('采样结果与当前 SVG 字节不一致，请重新读取。');
 return values as [number,number,number,number];
}
