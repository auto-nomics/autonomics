import { useEffect, useState } from "react";
import type { DebugClient } from "./debug-client";

type Frame = { id: number; name: string; line: number; column?: number; source?: { path?: string; name?: string } };
type Scope = { name: string; variablesReference: number; expensive?: boolean };
type Variable = { name: string; value: string; type?: string; variablesReference: number };

function Variables({ client, reference, depth = 0 }: { client: DebugClient; reference: number; depth?: number }) {
  const [variables, setVariables] = useState<Variable[]>();
  const [expanded, setExpanded] = useState<Set<number>>(new Set());
  const [error, setError] = useState("");
  useEffect(() => {
    let cancelled = false;
    setVariables(undefined); setExpanded(new Set()); setError("");
    void client.request("variables", { variablesReference: reference }).then(body => {
      if (!cancelled) setVariables(body?.variables ?? []);
    }).catch(cause => { if (!cancelled) setError(String(cause)); });
    return () => { cancelled = true; };
  }, [client, reference]);
  if (error) return <div role="alert" className="text-destructive">{error}</div>;
  if (!variables) return <div className="text-muted-foreground">加载变量…</div>;
  if (!variables.length) return <div className="text-muted-foreground">无变量</div>;
  return <ul className="space-y-0.5">{variables.map((variable, index) => <li key={`${variable.name}:${index}`}>
    <div className="flex min-w-0 items-start gap-1">
      {variable.variablesReference > 0 && depth < 10 ? <button type="button" aria-label={`${expanded.has(index) ? "折叠" : "展开"} ${variable.name}`} aria-expanded={expanded.has(index)} onClick={() => setExpanded(current => { const next = new Set(current); next.has(index) ? next.delete(index) : next.add(index); return next; })}>{expanded.has(index) ? "▾" : "▸"}</button> : <span className="w-2" />}
      <span className="break-all"><strong>{variable.name}</strong>: {variable.value}{variable.type && <span className="text-muted-foreground"> ({variable.type})</span>}</span>
    </div>
    {expanded.has(index) && variable.variablesReference > 0 && <div className="pl-4"><Variables client={client} reference={variable.variablesReference} depth={depth + 1} /></div>}
  </li>)}</ul>;
}

export function DebugInspector({ client, thread, onNavigate }: { client: DebugClient; thread: number; onNavigate: (path: string, line: number, column: number) => void }) {
  const [frames, setFrames] = useState<Frame[]>();
  const [selected, setSelected] = useState<number>();
  const [scopes, setScopes] = useState<Scope[]>();
  const [error, setError] = useState("");
  useEffect(() => {
    let cancelled = false;
    setFrames(undefined); setSelected(undefined); setScopes(undefined); setError("");
    void client.request("stackTrace", { threadId: thread, startFrame: 0, levels: 100 }).then(body => {
      if (cancelled) return;
      const next: Frame[] = body?.stackFrames ?? [];
      setFrames(next); setSelected(next[0]?.id);
    }).catch(cause => { if (!cancelled) setError(String(cause)); });
    return () => { cancelled = true; };
  }, [client, thread]);
  useEffect(() => {
    let cancelled = false;
    setScopes(undefined);
    if (selected !== undefined) void client.request("scopes", { frameId: selected }).then(body => {
      if (!cancelled) setScopes(body?.scopes ?? []);
    }).catch(cause => { if (!cancelled) setError(String(cause)); });
    return () => { cancelled = true; };
  }, [client, selected]);
  return <div className="mt-2 grid max-h-52 grid-cols-1 gap-3 overflow-auto sm:grid-cols-2" aria-label="调试检查器">
    <section aria-label="调用栈"><strong>调用栈</strong>{!frames ? <div>加载中…</div> : !frames.length ? <div>无调用栈</div> : <ul>{frames.map(frame => {
      const path = frame.source?.path;
      const relative = path?.startsWith("/work/") ? path.slice(6) : undefined;
      return <li key={frame.id} className="flex items-center gap-1"><button type="button" className={`min-w-0 truncate text-left ${selected === frame.id ? "font-semibold text-primary" : "hover:underline"}`} onClick={() => setSelected(frame.id)}>{frame.name} · {frame.source?.name ?? path ?? "未知文件"}:{frame.line}</button>{relative && <button type="button" className="shrink-0 underline" aria-label={`定位 ${frame.name} 源码`} onClick={() => onNavigate(relative, frame.line, frame.column ?? 1)}>定位</button>}</li>;
    })}</ul>}</section>
    <section aria-label="变量"><strong>变量</strong>{error && <div role="alert" className="text-destructive">{error}</div>}{selected !== undefined && (!scopes ? <div>加载中…</div> : !scopes.length ? <div>无变量作用域</div> : scopes.filter(scope => !scope.expensive).map((scope, index) => <div key={`${scope.name}:${index}`} className="mt-1"><div className="font-medium">{scope.name}</div><Variables client={client} reference={scope.variablesReference} /></div>))}</section>
  </div>;
}
