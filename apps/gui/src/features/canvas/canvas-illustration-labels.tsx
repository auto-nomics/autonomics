import { useState } from "react";
import { useTranslation } from "react-i18next";
import { Button } from "@/components/ui/button";
import { gatewayRequest } from "@/gateway";
export type IllustrationLabel = { id: string; text: string; x: number; y: number; width: number; fontSize: number };
export function CanvasIllustrationLabels({ labels, path, revision, unchanged, busy, onBusy, onError, onApplied }: {
  labels: IllustrationLabel[]; path: string; revision: number; unchanged: () => boolean; busy: boolean;
  onBusy: (value: boolean) => void; onError: (message: string) => void; onApplied: (id: string) => void;
}) {
  const { t } = useTranslation("canvas");
  return <section aria-label={t("illustrationLabels.sectionLabel")} className="mt-4 space-y-2 border-t pt-3"><p className="text-sm font-medium">{t("illustrationLabels.title")}</p><p className="text-xs text-muted-foreground">{t("illustrationLabels.description")}</p>{labels.map(label => <LabelLayout key={label.id} {...{label,path,revision,unchanged,busy,onBusy,onError,onApplied}} />)}</section>;
}
function LabelLayout({ label, path, revision, unchanged, busy, onBusy, onError, onApplied }: {
  label: IllustrationLabel; path: string; revision: number; unchanged: () => boolean; busy: boolean;
  onBusy: (value: boolean) => void; onError: (message: string) => void; onApplied: (id: string) => void;
}) {
  const { t } = useTranslation("canvas");
  const [values,setValues]=useState([label.x,label.y,label.width,label.fontSize]);
  const valid=values.every(n=>Number.isFinite(n)&&Math.abs(n)<=1e6)&&values[2]>=1&&values[2]<=20000&&label.text.length<=2048&&values[3]>=4&&values[3]<=200;
  const changed=values.some((value,index)=>value!==[label.x,label.y,label.width,label.fontSize][index]);
  return <article aria-label={t("illustrationLabels.itemLabel", { id: label.id })} className="space-y-2 rounded border p-2 text-xs"><p className="whitespace-pre-wrap break-all">{label.text}</p><div className="flex flex-wrap gap-2">{(["X","Y",t("illustrationLabels.wrapWidth"),t("illustrationLabels.fontSize")] as const).map((name,index)=><label key={name}>{name}<input type="number" aria-label={t("illustrationLabels.fieldLabel", { name })} className="ml-1 w-24 rounded border bg-background p-1" disabled={busy} value={Number.isFinite(values[index])?values[index]:""} onChange={event=>{const value=event.currentTarget.valueAsNumber;setValues(current=>current.map((old,i)=>i===index?value:old));}} /></label>)}</div>{!valid&&<p role="status" className="text-destructive">{t("illustrationLabels.invalid")}</p>}<Button size="sm" disabled={busy||!valid||!changed} onClick={async()=>{
    if(!valid||!changed)return;onBusy(true);onError("");
    try{
      if(!unchanged())throw new Error(t("illustrationLabels.canvasChanged"));
      const result=await gatewayRequest("POST",`${path}/operations`,{expected_revision:revision,summary:t("illustrationLabels.summary"),operations:[{op:"update",id:label.id,changes:{x:values[0],y:values[1],width:values[2]}},{op:"set_wrapped_text",id:label.id,text:label.text,max_width:values[2],font_size:values[3]}]}) as {id:string};
      if(!unchanged())throw new Error(t("common.canvasChangedDuringGeneration"));
      onApplied(result.id);
    }catch(cause){onError(String(cause));}finally{onBusy(false);}
  }}>{t("illustrationLabels.apply")}</Button></article>;
}
