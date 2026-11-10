import { useEffect, useState } from "react";
import { gatewayRequest } from "@/gateway";
export type ProposalNotice = { current_revision: number; total: number; current: number; stale: number; latest: { id: string; summary: string; summary_truncated: boolean; base_revision: number; parent_proposal_id: string | null } | null };
/** Notification polling transfers metadata only, including while the Agent tab is active. */
export function useProposalNotice(path: string) {
  const [notice, setNotice] = useState<ProposalNotice | null>(null);
  useEffect(() => {
    let cancelled = false, running = false;
    setNotice(null);
    const update = async () => {
      if (cancelled || running || document.hidden) return;
      running = true;
      try { const next = await gatewayRequest("GET", path) as ProposalNotice; if (!cancelled) setNotice(next); }
      catch { if (!cancelled) setNotice(null); }
      finally { running = false; }
    };
    void update();
    const timer = setInterval(() => void update(), 5000);
    window.addEventListener("focus", update); document.addEventListener("visibilitychange", update);
    return () => { cancelled = true; clearInterval(timer); window.removeEventListener("focus", update); document.removeEventListener("visibilitychange", update); };
  }, [path]);
  return notice;
}
