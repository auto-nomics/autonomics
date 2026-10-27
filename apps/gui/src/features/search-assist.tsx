import { useEffect, useState } from "react";
import { Button } from "@/components/ui/button";

export function readQueryHistory(): string[] {
  try { const value=JSON.parse(localStorage.getItem("autonomics.bib.searchHistory.v1")??"[]");return Array.isArray(value)?value.filter((item):item is string=>typeof item==="string"&&item.length<=16384).slice(0,20):[]; } catch {return [];}
}
export function useQueryHistory() {
  const [history,setHistory]=useState(readQueryHistory);
  useEffect(()=>{try{localStorage.setItem("autonomics.bib.searchHistory.v1",JSON.stringify(history));}catch{/* Optional history storage. */}},[history]);
  return {history,record:(text:string)=>{const query=text.trim();if(query)setHistory(current=>current[0]===query?current:[query,...current.filter(item=>item!==query)].slice(0,20));},clear:()=>setHistory([])};
}
export function SearchAssist({ history,onPick,onClear }: { history:string[];onPick:(query:string)=>void;onClear:()=>void }) {
  return <>
    <details className="relative"><summary className="cursor-pointer text-muted-foreground">Recent</summary><div className="absolute left-0 top-full z-20 mt-1 w-72 max-w-[85vw] rounded border bg-background p-2 shadow-lg">
      {!history.length&&<p className="text-muted-foreground">Successful searches appear here.</p>}
      <div className="max-h-60 overflow-auto">{history.map(query=><button key={query} className="block w-full truncate rounded p-1.5 text-left hover:bg-accent" title={query} onClick={event=>{onPick(query);event.currentTarget.closest("details")?.removeAttribute("open");}}>{query}</button>)}</div>
      {history.length>0&&<Button size="xs" variant="ghost" onClick={onClear}>Clear search history</Button>}
    </div></details>
    <details className="relative"><summary className="cursor-pointer text-muted-foreground">Search help</summary><div className="absolute left-0 top-full z-20 mt-1 w-80 max-w-[85vw] space-y-2 rounded border bg-background p-3 shadow-lg">
      <p>Use AND, OR, NOT and parentheses to combine conditions. Quote a phrase to keep words together.</p>
      <p><code>title:</code> · <code>author:</code> · <code>journal:</code> · <code>year:</code> · <code>tag:</code> · <code>fulltext:</code> · <code>note:</code></p>
      <button className="block text-left underline" onClick={()=>onPick('title:"gene editing" AND year:2020..2026')}>Try a title phrase and year range</button>
      <p>Enter a DOI directly for exact lookup. Explicit fields override the quick search field selection.</p>
      <p>Ctrl/Cmd+F focuses search. Down enters the results; arrow keys browse. Ctrl+Space selects an article.</p>
    </div></details>
  </>;
}
export type ResultColumns = {creator:boolean;year:boolean;journal:boolean;files:boolean};
export function readResultColumns():ResultColumns {
  const fallback={creator:true,year:true,journal:false,files:true};
  try{const value=JSON.parse(localStorage.getItem("autonomics.bib.columns.v1")??"null");return value?Object.fromEntries(Object.entries(fallback).map(([key,enabled])=>[key,typeof value[key]==="boolean"?value[key]:enabled])) as ResultColumns:fallback;}catch{return fallback;}
}
export function ColumnsPicker({columns,onChange,snippets,onSnippets}:{columns:ResultColumns;onChange:(value:ResultColumns)=>void;snippets:boolean;onSnippets:(value:boolean)=>void}) {
  return <details className="relative"><summary className="cursor-pointer text-muted-foreground">View</summary><div className="absolute right-0 top-full z-20 mt-1 min-w-40 space-y-2 rounded border bg-background p-2 shadow-lg">
    <label className="flex items-center gap-1"><input type="checkbox" checked={snippets} onChange={event=>onSnippets(event.target.checked)}/>Show match snippets</label>
    <hr/>{Object.entries({creator:"Author column",year:"Year column",journal:"Journal column",files:"Files column"}).map(([key,label])=><label key={key} className="flex items-center gap-1"><input type="checkbox" checked={columns[key as keyof ResultColumns]} onChange={event=>onChange({...columns,[key]:event.target.checked})}/>{label}</label>)}
  </div></details>;
}
