/** Bounded review hints for text rectangles, not glyph collision or scientific validation. */
type TextBox = {id:string;x:number;y:number;width:number;height:number};
function boxes(input: unknown) {
  const scene = input && typeof input === 'object' ? input as Record<string, unknown> : {};
  const all = Array.isArray(scene.elements) ? scene.elements : [];
  const texts: TextBox[] = [], omitted = new Set<string>();
  let skipped = 0, total = 0;
  for (const raw of all) {
    if (!raw || typeof raw !== 'object') continue;
    const e = raw as Record<string, unknown>;
    if (e.type !== 'text' || e.isDeleted === true) continue;
    total++;
    if (typeof e.id !== 'string' || e.id.length > 512 || e.angle !== 0 || e.opacity === 0 || ![e.x,e.y,e.width,e.height].every(v => typeof v === 'number' && Number.isFinite(v) && Math.abs(v) <= 1e6) || Number(e.width) <= 0 || Number(e.height) <= 0 || texts.length >= 1000) {
      skipped++; if (typeof e.id === 'string') omitted.add(e.id); continue;
    }
    texts.push({id:e.id,x:Number(e.x),y:Number(e.y),width:Number(e.width),height:Number(e.height)});
  }
  return {texts,omitted,skipped,total};
}
function intersects(a:TextBox,b:TextBox) {
  return a.id !== b.id && Math.min(a.x+a.width,b.x+b.width)-Math.max(a.x,b.x)>1 && Math.min(a.y+a.height,b.y+b.height)-Math.max(a.y,b.y)>1;
}
function visit(texts:TextBox[],fn:(a:TextBox,b:TextBox)=>void) {
  for(let i=0;i<texts.length;i++)for(let j=i+1;j<texts.length;j++)if(intersects(texts[i],texts[j]))fn(texts[i],texts[j]);
}
export function textOverlaps(input: unknown) {
  const {texts,total,skipped}=boxes(input);
  const pairs: {first:string;second:string}[] = []; let count = 0;
  visit(texts,(a,b)=>{count++;if(pairs.length<50)pairs.push({first:a.id,second:b.id});});
  return {pairs,count,total,checked:texts.length,skipped,truncated:count>pairs.length};
}
/** Classify pairs by geometry in both scenes; capped output never drives classification. */
export function textOverlapChanges(before:unknown,after:unknown) {
  const a=boxes(before),b=boxes(after), ai=new Map(a.texts.map(e=>[e.id,e])),bi=new Map(b.texts.map(e=>[e.id,e]));
  const counts={introduced:0,persisting:0,resolved:0,unknown:0};
  const pairs:{first:string;second:string;status:keyof typeof counts}[]=[];
  const exists=(x:TextBox,y:TextBox,index:Map<string,TextBox>,omitted:Set<string>):boolean|null=>{
    if(omitted.has(x.id)||omitted.has(y.id))return null;
    const left=index.get(x.id),right=index.get(y.id);
    return left&&right?intersects(left,right):false;
  };
  const add=(x:TextBox,y:TextBox,status:keyof typeof counts)=>{counts[status]++;if(pairs.length<50)pairs.push({first:x.id,second:y.id,status});};
  visit(b.texts,(x,y)=>{const prior=exists(x,y,ai,a.omitted);add(x,y,prior===null?'unknown':prior?'persisting':'introduced');});
  visit(a.texts,(x,y)=>{const current=exists(x,y,bi,b.omitted);if(current!==true)add(x,y,current===null?'unknown':'resolved');});
  return {counts,pairs,before:{total:a.total,checked:a.texts.length,skipped:a.skipped},after:{total:b.total,checked:b.texts.length,skipped:b.skipped},truncated:Object.values(counts).reduce((n,v)=>n+v,0)>pairs.length};
}
