import { useEffect, useRef, useState } from "react";
import { gatewayRequest } from "@/gateway";
import { Button } from "@/components/ui/button";
type Child = { id: string; summary: string; summary_truncated?: boolean; base_revision: number; parent_proposal_id?: string };
type Page = { items: Child[]; next_before: string | null };
export function CanvasCandidateChildren({ path, busy, onSelect }: { path: string; busy: boolean; onSelect: (child: Child) => Promise<void> }) {
  const [items, setItems] = useState<Child[]>([]), [nextBefore, setNextBefore] = useState<string | null>(null), [loading, setLoading] = useState(false), [loaded, setLoaded] = useState(false), [error, setError] = useState("");
  const active = useRef(false), serial = useRef(0);
  async function load(before?: string) {
    const request = ++serial.current; setLoading(true); setError("");
    try {
      const page = await gatewayRequest("GET", `${path}/children?limit=20${before ? `&before=${encodeURIComponent(before)}` : ""}`, undefined, { signal: AbortSignal.timeout(5000) }) as Page;
      if (!active.current || serial.current !== request) return;
      setItems(current => before ? [...current, ...page.items.filter(item => !current.some(previous => previous.id === item.id))] : page.items);
      setNextBefore(page.next_before); setLoaded(true);
    } catch (cause) { if (active.current && serial.current === request) setError(String(cause)); }
    finally { if (active.current && serial.current === request) setLoading(false); }
  }
  useEffect(() => { active.current = true; void load(); return () => { active.current = false; serial.current++; }; }, [path]);
  return <details aria-label="候选后续修订" className="shrink-0 border-b text-xs"><summary className="cursor-pointer p-2">后续修订：{!loaded ? "核对中" : `${items.length}${nextBefore ? "+" : ""} 份待审直接修订`}{error ? " · 查询未完成" : ""}</summary><div className="max-h-40 space-y-2 overflow-auto p-2">
    <p>此处列出当前候选的直接修订；逐步打开可查看后面的版本。多个分支分别审阅，不会自动合并。</p>
    {loaded && !items.length && !error && <p>暂无待审的直接修订。</p>}
    {items.map(child => <Button key={child.id} data-child-proposal-id={child.id} size="sm" variant="outline" className="mr-2 h-auto max-w-full whitespace-normal text-left" disabled={busy || loading} onClick={() => void onSelect(child)}>查看后续修订：{child.summary}{child.summary_truncated ? "…" : ""}</Button>)}
    {nextBefore && <Button size="sm" variant="outline" disabled={busy || loading} onClick={() => void load(nextBefore)}>加载更多后续修订</Button>}
    <Button size="sm" variant="outline" disabled={busy || loading} onClick={() => void load()}>刷新后续修订</Button>
    {error && <p role="alert" className="text-destructive">未能核对完整的后续修订，请刷新重试。{error}</p>}
  </div></details>;
}
