/** Compatibility for content-free legacy values; preserves all real references and data. */
export function normalizeEmptyNativeFields<T extends {elements?:readonly unknown[]|null}>(document:T):T{
 let changed=false;
 const elements=document.elements?.map(raw=>{
  if(!raw||typeof raw!=='object'||Array.isArray(raw))return raw;
  const element=raw as Record<string,unknown>,patch:Record<string,unknown>={};
  for(const field of ['boundElements','groupIds'])if(element[field]==='')patch[field]=[];
  for(const field of ['containerId','frameId'])if(element[field]==='')patch[field]=null;
  if(!Object.keys(patch).length)return raw;
  changed=true;return {...element,...patch};
 });
 return changed?{...document,elements}:document;
}
