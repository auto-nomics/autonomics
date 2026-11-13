export type PlanDifference = { field: string; before: string; after: string };
function canonical(value: unknown): unknown {
  if (Array.isArray(value)) return value.map(canonical);
  if (value && typeof value === "object") return Object.fromEntries(Object.entries(value).sort(([a],[b])=>a.localeCompare(b)).map(([key,item])=>[key,canonical(item)]));
  return value;
}
export function samePlanValue(a:unknown,b:unknown):boolean { return JSON.stringify(canonical(a))===JSON.stringify(canonical(b)); }
export function planValuePreview(value: unknown, field: string): string {
  if(value!==undefined&&/dataurl/i.test(field))return "[数据已隐藏]";
  const clean=(item:unknown):unknown=>{
    if(typeof item==="string")return item.startsWith("data:")?"[数据已隐藏]":item;
    if(Array.isArray(item))return item.map(clean);
    if(item&&typeof item==="object")return Object.fromEntries(Object.entries(item).map(([key,value])=>[key,/dataurl/i.test(key)?"[数据已隐藏]":clean(value)]));
    return item;
  };
  const text=value===undefined?"未设置":typeof value==="string"?String(clean(value)):JSON.stringify(clean(value));
  return text.length>600?`${text.slice(0,600)}…（已截断）`:text;
}
// Structural array changes are shown as a whole, rather than pretending that
// reordered node/question identities are edits to the old indexed object.
export function planDifferences(before: unknown, after: unknown) {
  const fields:PlanDifference[]=[];let total=0;
  const add=(field:string,a:unknown,b:unknown)=>{total++;if(fields.length<40)fields.push({field,before:planValuePreview(a,field),after:planValuePreview(b,field)});};
  const walk=(a:unknown,b:unknown,path:string)=>{
    if(samePlanValue(a,b))return;
    if(Array.isArray(a)&&Array.isArray(b)){
      const identity=(value:unknown)=>value&&typeof value==="object"?JSON.stringify([('id' in value)?value.id:null,('from' in value)?value.from:null,('to' in value)?value.to:null]):null;
      if(a.length!==b.length||a.some((item,i)=>identity(item)!==identity(b[i]))){add(path,a,b);return;}
      a.forEach((item,i)=>walk(item,b[i],`${path}[${i}]`));return;
    }
    if(a&&b&&typeof a==="object"&&typeof b==="object"&&!Array.isArray(a)&&!Array.isArray(b)){
      const first=a as Record<string,unknown>,second=b as Record<string,unknown>;
      for(const key of [...new Set([...Object.keys(first),...Object.keys(second)])].sort())walk(first[key],second[key],path?`${path}.${key}`:key);
      return;
    }
    add(path,a,b);
  };
  walk(before,after,"");return {fields,total,omitted:total-fields.length};
}
export function planDifferenceLabel(field:string):string {
  const roots:Record<string,string>={nodes:"组件",edges:"关系",panels:"分面",questions:"问题",assumptions:"假设",message:"核心图意",audience:"受众",layout:"布局",relationship_note:"关系说明"};
  const names:Record<string,string>={label:"标签",title:"标题",evidence:"依据",from:"起点",to:"终点",direction:"方向",inhibitory:"抑制",line_style:"线型",question:"问题文本",answer:"答案",blocking:"生成前必须确认",panel:"所属分面",column:"列",row:"行",shape:"形状",id:"标识"};
  return field.replace(/^[a-z_]+/,key=>roots[key]??key).replace(/\[(\d+)\]/g,(_,index)=>` ${Number(index)+1}`).replace(/\.([a-z_]+)/g,(_,key)=>` · ${names[key]??key}`);
}
