import { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { Button } from "@/components/ui/button";

export function readQueryHistory(key = "autonomics.bib.searchHistory.v1"): string[] {
  try { const value=JSON.parse(localStorage.getItem(key)??"[]");return Array.isArray(value)?value.filter((item):item is string=>typeof item==="string"&&item.length<=16384).slice(0,20):[]; } catch {return [];}
}
export function useQueryHistory(key = "autonomics.bib.searchHistory.v1") {
  const [history,setHistory]=useState(()=>readQueryHistory(key));
  useEffect(()=>{try{localStorage.setItem(key,JSON.stringify(history));}catch{/* Optional history storage. */}},[history,key]);
  return {history,record:(text:string)=>{const query=text.trim();if(query)setHistory(current=>current[0]===query?current:[query,...current.filter(item=>item!==query)].slice(0,20));},clear:()=>setHistory([])};
}
export function SearchAssist({ history,onPick,onClear,topic = false }: { history:string[];onPick:(query:string)=>void;onClear:()=>void;topic?:boolean }) {
  const { t } = useTranslation("bibliography");
  return <>
    <details className="relative"><summary className="cursor-pointer text-muted-foreground">{t("searchAssist.recent")}</summary><div className="absolute left-0 top-full z-20 mt-1 w-72 max-w-[85vw] rounded border bg-background p-2 shadow-lg">
      {!history.length&&<p className="text-muted-foreground">{t("searchAssist.recentEmpty")}</p>}
      <div className="max-h-60 overflow-auto">{history.map(query=><button key={query} className="block w-full truncate rounded p-1.5 text-left hover:bg-accent" title={query} onClick={event=>{onPick(query);event.currentTarget.closest("details")?.removeAttribute("open");}}>{query}</button>)}</div>
      {history.length>0&&<Button size="xs" variant="ghost" onClick={onClear}>{t("searchAssist.clearHistory")}</Button>}
    </div></details>
    <details className="relative"><summary className="cursor-pointer text-muted-foreground">{t("searchAssist.help")}</summary><div className="absolute left-0 top-full z-20 mt-1 w-80 max-w-[85vw] space-y-2 rounded border bg-background p-3 shadow-lg">
      {topic ? <>
        <p>{t("searchAssist.topicHelp1")}</p>
        <p>{t("searchAssist.topicHelp2")}</p>
        <p>{t("searchAssist.topicHelp3")}</p>
      </> : <>
      <p>{t("searchAssist.help1")}</p>
      <p><code>title:</code> · <code>author:</code> · <code>journal:</code> · <code>year:</code> · <code>tag:</code> · <code>fulltext:</code> · <code>note:</code></p>
      <button className="block text-left underline" onClick={()=>onPick('title:"gene editing" AND year:2020..2026')}>{t("searchAssist.tryExample")}</button>
      <p>{t("searchAssist.help2")}</p>
      </>}
      <p>{t("searchAssist.keys")}</p>
    </div></details>
  </>;
}
export type ResultColumns = {creator:boolean;year:boolean;journal:boolean;files:boolean};
export function readResultColumns():ResultColumns {
  const fallback={creator:true,year:true,journal:false,files:true};
  try{const value=JSON.parse(localStorage.getItem("autonomics.bib.columns.v1")??"null");return value?Object.fromEntries(Object.entries(fallback).map(([key,enabled])=>[key,typeof value[key]==="boolean"?value[key]:enabled])) as ResultColumns:fallback;}catch{return fallback;}
}
export function ColumnsPicker({columns,onChange,snippets,onSnippets}:{columns:ResultColumns;onChange:(value:ResultColumns)=>void;snippets:boolean;onSnippets:(value:boolean)=>void}) {
  const { t } = useTranslation("bibliography");
  return <details className="relative"><summary className="cursor-pointer text-muted-foreground">{t("searchAssist.view")}</summary><div className="absolute right-0 top-full z-20 mt-1 min-w-40 space-y-2 rounded border bg-background p-2 shadow-lg">
    <label className="flex items-center gap-1"><input type="checkbox" checked={snippets} onChange={event=>onSnippets(event.target.checked)}/>{t("searchAssist.showSnippets")}</label>
    <hr/>{(["creator","year","journal","files"] as const).map(key=><label key={key} className="flex items-center gap-1"><input type="checkbox" checked={columns[key]} onChange={event=>onChange({...columns,[key]:event.target.checked})}/>{t(`searchAssist.column_${key}`)}</label>)}
  </div></details>;
}
