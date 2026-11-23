/** Runtime-only provenance copied by Excalidraw 0.18's deepCopyElement.
 * toJSON deliberately omits the complete marker from scene/clipboard exports.
 */
import i18n from "@/i18n";
type Element = {readonly id:string;readonly groupIds:readonly string[];readonly customData?:Record<string,unknown>;readonly fileId?:string|null;readonly type?:string};
const markerKey=`__autonomics_copy_${Math.random().toString(36).slice(2)}`;
const omit=()=>undefined;
type Marker={id:string;groups:string[];toJSON:typeof omit;hadData?:boolean};
function marker(element:Element):Marker|undefined {
  const value=(Reflect.get(element,markerKey)??element.customData?.[markerKey]) as Marker|undefined;
  return value&&value.toJSON===omit&&typeof value.id==='string'&&Array.isArray(value.groups)?value:undefined;
}
export function stampNativeIdentities<T extends Element>(elements:readonly T[]) {
  for(const element of elements){
    const previous=marker(element);
    if(previous?.id===element.id&&previous.groups.length===element.groupIds.length&&previous.groups.every((g,i)=>g===element.groupIds[i]))continue;
    if(Object.isExtensible(element))Object.defineProperty(element,markerKey,{enumerable:true,configurable:true,writable:true,value:{id:element.id,groups:[...element.groupIds],toJSON:omit}});
  }
}
export function repairNativeDuplicate<T extends Element>(next:readonly T[],previous:readonly T[]):T[] {
  const previousIds=new Set(previous.map(e=>e.id)),copies=next.filter(e=>!previousIds.has(e.id));
  const controlled=copies.some(e=>['semanticAnchorId','visualAssetElementId','visualAssetGroupId','visualLabelIds','inset','annotationKind'].some(key=>e.customData?.[key]!=null));
  if(!controlled)return [...next];
  if(copies.some(e=>e.customData?.inset!=null||e.customData?.annotationKind!=null))throw new Error(i18n.t("canvas:nativeDuplicate.copyUnsupported"));
  const identities=new Map<string,string>(),groups=new Map<string,string>();
  for(const copy of copies){
    const source=marker(copy);
    if(!source||identities.has(source.id))throw new Error(i18n.t("canvas:nativeDuplicate.missingIdentity"));
    identities.set(source.id,copy.id);
    if(source.groups.length!==copy.groupIds.length)throw new Error(i18n.t("canvas:nativeDuplicate.groupChanged"));
    source.groups.forEach((group,i)=>{const target=copy.groupIds[i];if(groups.has(group)&&groups.get(group)!==target)throw new Error(i18n.t("canvas:nativeDuplicate.groupAmbiguous"));groups.set(group,target);});
  }
  const result=next.map(element=>{
    if(previousIds.has(element.id)||!element.customData)return element;
    const identity=marker(element),data={...element.customData};
    delete data[markerKey];
    for(const key of ['semanticAnchorId','visualAssetElementId']){
      if(data[key]==null)continue;
      if(typeof data[key]!=='string')throw new Error(i18n.t("canvas:nativeDuplicate.invalidOwnership"));
      const target=identities.get(data[key]);
      if(target)data[key]=target;else delete data[key];
    }
    if(data.visualLabelIds!=null){if(!Array.isArray(data.visualLabelIds)||!data.visualLabelIds.every(id=>typeof id==='string'))throw new Error(i18n.t("canvas:nativeDuplicate.invalidLabels"));data.visualLabelIds=data.visualLabelIds.flatMap(id=>typeof id==='string'&&identities.has(id)?[identities.get(id)!]:[]);}
    if(data.visualAssetGroupId!=null){if(typeof data.visualAssetGroupId!=='string')throw new Error(i18n.t("canvas:nativeDuplicate.invalidAssetGroup"));const target=groups.get(data.visualAssetGroupId);if(target)data.visualAssetGroupId=target;else if(data.visualAssetElementId)throw new Error(i18n.t("canvas:nativeDuplicate.assetGroupAmbiguous"));else delete data.visualAssetGroupId;}
    // Alt-drag fixes native bindings after this callback using these same new objects.
    Object.assign(element,{customData:identity?.hadData===false&&!Object.keys(data).length?undefined:data});
    return element;
  });
  stampNativeIdentities(result);return result;
}

/** Paste provenance lives inside customData only across native restoreElements. */
export function prepareNativePaste<T extends Element,F extends {dataURL:unknown;id?:string}>(elements:readonly T[],files:Record<string,F>|undefined,existing:Record<string,{dataURL:unknown}>) {
  if(elements.some(e=>e.customData?.inset!=null||e.customData?.annotationKind!=null))throw new Error(i18n.t("canvas:nativeDuplicate.pasteUnsupported"));
  const controlled=elements.some(e=>['semanticAnchorId','visualAssetElementId','visualAssetGroupId','visualLabelIds'].some(key=>e.customData?.[key]!=null));
  if(controlled&&new Set(elements.map(e=>e.id)).size!==elements.length)throw new Error(i18n.t("canvas:nativeDuplicate.pasteDuplicateIds"));
  const remap=new Map<string,string>(),nextFiles={...files};
  for(const [id,file] of Object.entries(files??{})){
    if(typeof file.dataURL!=='string'||!/^data:image\//i.test(file.dataURL))throw new Error(i18n.t("canvas:nativeDuplicate.pasteInvalidImage"));;
    if(existing[id]&&existing[id].dataURL!==file.dataURL){
      let target:string;do{target=`paste_${Array.from(crypto.getRandomValues(new Uint32Array(4)),v=>v.toString(16)).join('_')}`;}while(existing[target]||nextFiles[target]);
      remap.set(id,target);delete nextFiles[id];nextFiles[target]={...file,id:target};
    }
  }
  const prepared=elements.map(e=>{
    if(e.type==='image'&&(!e.fileId||!files?.[e.fileId]))throw new Error(i18n.t("canvas:nativeDuplicate.pasteMissingImage"));;
    const copy={...e,...(e.fileId&&remap.has(e.fileId)?{fileId:remap.get(e.fileId)!}:{})};
    if(controlled){
      if(!e.id||!Array.isArray(e.groupIds)||!e.groupIds.every(g=>typeof g==='string')||(e.customData!=null&&(typeof e.customData!=='object'||Array.isArray(e.customData))))throw new Error(i18n.t("canvas:nativeDuplicate.pasteIncompleteIdentity"));;
      Object.assign(copy,{customData:{...e.customData,[markerKey]:{id:e.id,groups:[...e.groupIds],hadData:e.customData!=null,toJSON:omit}}});
    }
    return copy;
  });
  return {elements:prepared,files:files?nextFiles:undefined};
}

/** Native Alt duplication expands groups/frames; protect the complete selected component. */
export function hasProtectedCopyTarget<T extends Element & {readonly frameId?:string|null}>(elements:readonly T[],selected:Readonly<Record<string,boolean>>,hitId?:string) {
  const protectedIds=new Set(elements.filter(e=>e.customData?.inset!=null||e.customData?.annotationKind!=null).map(e=>e.id));
  if(!protectedIds.size)return false;
  const targets=new Set(Object.entries(selected).filter(([,value])=>value===true).map(([id])=>id));if(hitId)targets.add(hitId);
  let changed=true;
  while(changed){
    changed=false;const groups=new Set(elements.filter(e=>targets.has(e.id)).flatMap(e=>[...e.groupIds]));
    for(const e of elements)if(!targets.has(e.id)&&((e.frameId&&targets.has(e.frameId))||e.groupIds.some(g=>groups.has(g)))){targets.add(e.id);changed=true;}
  }
  return [...targets].some(id=>protectedIds.has(id));
}
