import type { ScientificBrief } from "./canvas-planning";
import { samePlanValue } from "./canvas-plan-diff";
export type PlanMergeConflict={field:string;base:unknown;local:unknown;remote:unknown};
// Merge saved-base/local-draft/latest-saved values. Concurrent structural list
// changes are atomic conflicts; never remap array indices across identities.
export function mergePlanDraft(base:ScientificBrief,local:ScientificBrief,remote:ScientificBrief,resolutions:Record<string,"local"|"remote">={}):{document?:ScientificBrief;conflicts:string[];conflictCount:number;details:PlanMergeConflict[]} {
  const conflicts:string[]=[];const details:PlanMergeConflict[]=[];let conflictCount=0;
  const conflict=(path:string,base:unknown,local:unknown,remote:unknown)=>{const choice=Object.hasOwn(resolutions,path)?resolutions[path]:undefined;if(choice==="local")return structuredClone(local);if(choice==="remote")return structuredClone(remote);conflictCount++;if(conflicts.length<40){conflicts.push(path);details.push({field:path,base:structuredClone(base),local:structuredClone(local),remote:structuredClone(remote)});}return undefined;};
  const object=(v:unknown):v is Record<string,unknown>=>Boolean(v)&&typeof v==="object"&&!Array.isArray(v);
  const identity=(v:unknown)=>object(v)?JSON.stringify([v.id??null,v.from??null,v.to??null]):null;
  const merge=(a:unknown,b:unknown,c:unknown,path:string):unknown=>{
    if(samePlanValue(b,c))return structuredClone(b);
    if(samePlanValue(a,b))return structuredClone(c);
    if(samePlanValue(a,c))return structuredClone(b);
    if(Array.isArray(a)&&Array.isArray(b)&&Array.isArray(c)){
      if(a.length!==b.length||a.length!==c.length||new Set(a.map(identity)).size!==a.length||a.some((v,i)=>!object(v)||identity(v)!==identity(b[i])||identity(v)!==identity(c[i])))return conflict(path,a,b,c);
      return a.map((v,i)=>merge(v,b[i],c[i],`${path}[${i}]`));
    }
    if(object(a)&&object(b)&&object(c)){
      const result:Record<string,unknown>={};
      for(const key of [...new Set([...Object.keys(a),...Object.keys(b),...Object.keys(c)])].sort()){
        const value=merge(a[key],b[key],c[key],path?`${path}.${key}`:key);if(value!==undefined)result[key]=value;
      }
      return result;
    }
    return conflict(path,a,b,c);
  };
  const document=merge(base,local,remote,"") as ScientificBrief;
  return conflictCount?{conflicts,conflictCount,details}:{document,conflicts,conflictCount,details};
}
