import { useState } from "react";
import { Button } from "@/components/ui/button";
import { gatewayRequest } from "@/gateway";
export type IllustrationLabel = { id: string; text: string; x: number; y: number; width: number; fontSize: number };
export function CanvasIllustrationLabels({ labels, path, revision, unchanged, busy, onBusy, onError, onApplied }: {
  labels: IllustrationLabel[]; path: string; revision: number; unchanged: () => boolean; busy: boolean;
  onBusy: (value: boolean) => void; onError: (message: string) => void; onApplied: (id: string) => void;
}) {
  return <section aria-label="插图标签布局" className="mt-4 space-y-2 border-t pt-3"><p className="text-sm font-medium">插图标签布局</p><p className="text-xs text-muted-foreground">保留完整标签文字，调整位置、换行宽度和字号。只修改所选标签，图片和连线保留；修改后请检查文字是否遮挡其他对象。</p>{labels.map(label => <LabelLayout key={label.id} {...{label,path,revision,unchanged,busy,onBusy,onError,onApplied}} />)}</section>;
}
function LabelLayout({ label, path, revision, unchanged, busy, onBusy, onError, onApplied }: {
  label: IllustrationLabel; path: string; revision: number; unchanged: () => boolean; busy: boolean;
  onBusy: (value: boolean) => void; onError: (message: string) => void; onApplied: (id: string) => void;
}) {
  const [values,setValues]=useState([label.x,label.y,label.width,label.fontSize]);
  const valid=values.every(n=>Number.isFinite(n)&&Math.abs(n)<=1e6)&&values[2]>=1&&values[2]<=20000&&label.text.length<=2048&&values[3]>=4&&values[3]<=200;
  const changed=values.some((value,index)=>value!==[label.x,label.y,label.width,label.fontSize][index]);
  return <article aria-label={`标签布局 ${label.id}`} className="space-y-2 rounded border p-2 text-xs"><p className="whitespace-pre-wrap break-all">{label.text}</p><div className="flex flex-wrap gap-2">{(["X","Y","换行宽度","字号"] as const).map((name,index)=><label key={name}>{name}<input type="number" aria-label={`标签${name}`} className="ml-1 w-24 rounded border bg-background p-1" disabled={busy} value={Number.isFinite(values[index])?values[index]:""} onChange={event=>{const value=event.currentTarget.valueAsNumber;setValues(current=>current.map((old,i)=>i===index?value:old));}} /></label>)}</div>{!valid&&<p role="status" className="text-destructive">请填写有效位置、1–20000 的宽度和 4–200 的字号；标签须不超过 2048 字。</p>}<Button size="sm" disabled={busy||!valid||!changed} onClick={async()=>{
    if(!valid||!changed)return;onBusy(true);onError("");
    try{
      if(!unchanged())throw new Error("画布发生变化，请重新打开素材编辑。");
      const result=await gatewayRequest("POST",`${path}/operations`,{expected_revision:revision,summary:"调整插图标签布局",operations:[{op:"update",id:label.id,changes:{x:values[0],y:values[1],width:values[2]}},{op:"set_wrapped_text",id:label.id,text:label.text,max_width:values[2],font_size:values[3]}]}) as {id:string};
      if(!unchanged())throw new Error("生成修改期间画布发生变化。修改已保留，请保存后核对版本。");
      onApplied(result.id);
    }catch(cause){onError(String(cause));}finally{onBusy(false);}
  }}>应用标签布局修改</Button></article>;
}
