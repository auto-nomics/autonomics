import { useCallback, useEffect, useState } from "react";
import { gatewayRequest, gatewayRequestOptional } from "@/gateway";
import { renderMarkdown } from "@/lib/markdown";
import type {
  KmsEvidence,
  KmsKnowledgeNode,
  KmsProposal,
  KmsRow,
  KmsWorker,
  SkillLibraryDetail,
  SkillLibraryRow,
} from "@/types";
import { Button } from "@/components/ui/button";
import { Card } from "@/components/ui/card";
import { ScrollArea } from "@/components/ui/scroll-area";
import { Separator } from "@/components/ui/separator";
import { Tabs, TabsContent, TabsList, TabsTrigger } from "@/components/ui/tabs";
import { cn } from "@/lib/utils";

function formatUnixTime(value: number | null | undefined) {
  if (!value) return "—";
  return new Date(value < 10_000_000_000 ? value * 1000 : value).toLocaleString();
}

function mergeRows(library: SkillLibraryRow[], proposals: KmsProposal[]): KmsRow[] {
  const rows = library.map((skill) => {
    const proposal = proposals.find((item) => item.skill_name === skill.name) ?? null;
    return {
      key: skill.name,
      name: skill.name,
      tier: skill.tier,
      description: skill.description,
      tags: skill.tags,
      body: "",
      installed: skill.installed,
      proposal_status: proposal?.status ?? skill.proposal_status,
      proposal_update: proposal ? skill.installed : skill.proposal_update,
      proposal,
    };
  });
  const seen = new Set(rows.map((row) => row.name));
  for (const proposal of proposals) {
    if (!seen.has(proposal.skill_name)) {
      rows.push({
        key: proposal.skill_name,
        name: proposal.skill_name,
        tier: "proposed",
        description: proposal.title,
        tags: [],
        body: proposal.body_md,
        installed: false,
        proposal_status: proposal.status,
        proposal_update: false,
        proposal,
      });
    }
  }
  return rows.sort((left, right) => {
    const pending = (row: KmsRow) => row.proposal?.status !== "pending" ? 1 : 0;
    return pending(left) - pending(right) || left.name.localeCompare(right.name);
  });
}

export function KmsView() {
  const [workers, setWorkers] = useState<KmsWorker[]>([]);
  const [knowledge, setKnowledge] = useState<KmsKnowledgeNode[]>([]);
  const [rows, setRows] = useState<KmsRow[]>([]);
  const [proposals, setProposals] = useState<KmsProposal[]>([]);
  const [evidence, setEvidence] = useState<KmsEvidence[]>([]);
  const [skillDetail, setSkillDetail] = useState<SkillLibraryDetail | null>(null);
  const [selectedSkill, setSelectedSkill] = useState<string | null>(null);
  const [selectedKnowledge, setSelectedKnowledge] = useState<string | null>(null);
  const [selectedEvidence, setSelectedEvidence] = useState<string | null>(null);
  const [loading, setLoading] = useState(true);
  const [actionBusy, setActionBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [report, setReport] = useState<string | null>(null);

  const loadSkillDetail = useCallback(async (name: string | null) => {
    if (!name) {
      setSkillDetail(null);
      return;
    }
    try {
      setSkillDetail(await gatewayRequestOptional(`/skills/library/${encodeURIComponent(name)}`));
    } catch (cause) {
      setSkillDetail(null);
      setError(String(cause));
    }
  }, []);

  const load = useCallback(async () => {
    setLoading(true);
    setError(null);
    const results = await Promise.allSettled([
      gatewayRequest("GET", "/kms/workers"),
      gatewayRequest("GET", "/skills/library"),
      gatewayRequestOptional("/kms/knowledge"),
      gatewayRequestOptional("/kms/skill-proposals"),
      gatewayRequestOptional("/kms/evidence"),
    ]);
    const failures = results
      .slice(0, 2)
      .filter((result) => result.status === "rejected")
      .map((result) => String((result as PromiseRejectedResult).reason));
    const nextWorkers = results[0].status === "fulfilled" ? results[0].value as KmsWorker[] : [];
    const library = results[1].status === "fulfilled" ? results[1].value as SkillLibraryRow[] : [];
    const nextKnowledge = results[2].status === "fulfilled" && results[2].value ? [...results[2].value as KmsKnowledgeNode[]] : [];
    const nextProposals = results[3].status === "fulfilled" && results[3].value ? results[3].value as KmsProposal[] : [];
    const nextEvidence = results[4].status === "fulfilled" && results[4].value ? results[4].value as KmsEvidence[] : [];
    nextKnowledge.sort((left, right) => left.path.localeCompare(right.path));
    const nextRows = mergeRows(library, nextProposals);
    setWorkers(nextWorkers);
    setKnowledge(nextKnowledge);
    setProposals(nextProposals);
    setEvidence(nextEvidence);
    setRows(nextRows);
    setSelectedSkill((current) => {
      const valid = nextRows.some((row) => row.name === current) ? current : null;
      return valid ?? nextRows.find((row) => row.proposal?.status === "pending")?.name ?? nextRows[0]?.name ?? null;
    });
    setSelectedKnowledge((current) => nextKnowledge.some((node) => node.id === current) ? current : nextKnowledge[0]?.id ?? null);
    setSelectedEvidence((current) => nextEvidence.some((item) => item.id === current) ? current : nextEvidence[0]?.id ?? null);
    setError(failures.length ? failures.join("; ") : null);
    setLoading(false);
  }, []);

  useEffect(() => {
    void load();
  }, [load]);

  useEffect(() => {
    void loadSkillDetail(selectedSkill);
  }, [loadSkillDetail, selectedSkill]);

  const decide = async (approved: boolean) => {
    const proposal = rows.find((row) => row.name === selectedSkill)?.proposal;
    if (!proposal || proposal.status !== "pending" || actionBusy) return;
    setActionBusy(true);
    try {
      const response = await gatewayRequest(
        "POST",
        `/kms/skill-proposals/${proposal.id}/${approved ? "approve" : "reject"}`,
      );
      setReport(`${approved ? "approved" : "rejected"} ${response.skill_name}${response.skill_version_id ? ` (${response.skill_version_id})` : ""}`);
      await load();
    } catch (cause) {
      setError(String(cause));
    } finally {
      setActionBusy(false);
    }
  };

  const pending = proposals.filter((item) => item.status === "pending").length;
  const approved = proposals.filter((item) => item.status === "approved").length;
  const auto = rows.filter((row) => row.tags.includes("auto")).length;
  const running = workers.filter((worker) => worker.running).length;
  const proposer = workers.find((worker) => worker.worker === "propose_skill_revision");
  const row = rows.find((item) => item.name === selectedSkill);
  const detail = skillDetail?.name === row?.name ? skillDetail : null;
  const knowledgeNode = knowledge.find((node) => node.id === selectedKnowledge);
  const evidenceItem = evidence.find((item) => item.id === selectedEvidence);

  return (
    <div className="flex min-h-0 flex-1 flex-col gap-3">
      <Card className="p-3 text-xs text-muted-foreground">
        <span>
          observations {evidence.length} · skills {auto} auto · proposals {approved} approved / {pending} pending · workers {running}/{workers.length} running · last cycle {formatUnixTime(proposer?.last_run_at)}
        </span>
        {report && <div className="mt-1 text-xs text-primary">last decision: {report}</div>}
      </Card>

      {loading && <Card className="p-3 text-sm text-muted-foreground">Loading KMS…</Card>}
      {error && <Card className="border-destructive/40 p-3 text-sm text-destructive">{error}</Card>}

      <Tabs defaultValue="skills" className="grid min-h-0 flex-1 grid-rows-[auto_minmax(0,1fr)]">
        <div className="flex items-center justify-between">
          <TabsList>
            <TabsTrigger value="skills">Skills ({rows.length})</TabsTrigger>
            <TabsTrigger value="wiki">Wiki ({knowledge.length})</TabsTrigger>
            <TabsTrigger value="evidence">Evidence ({evidence.length})</TabsTrigger>
          </TabsList>
          <Button size="sm" variant="secondary" onClick={() => void load()}>Refresh</Button>
        </div>
        <TabsContent value="skills" className="grid min-h-0 grid-cols-[320px_minmax(0,1fr)] gap-3">
          <Card className="min-h-0 p-0!">
            <ScrollArea className="h-full p-1.5">
            {rows.map((item) => (
              <button
                key={item.key}
                type="button"
                onClick={() => setSelectedSkill(item.name)}
                className={cn("mb-1 w-full rounded-md px-2 py-2 text-left hover:bg-accent", selectedSkill === item.name && "bg-accent")}
              >
                <span className="block truncate text-sm font-medium">{item.name}</span>
                <span className="block truncate text-xs text-muted-foreground">
                  [{item.tier[0]}]{item.proposal_status ? ` · ${item.proposal_status}${item.proposal_update ? " ↑" : ""}` : ""}
                </span>
              </button>
            ))}
            </ScrollArea>
          </Card>
          <Card className="min-h-0 p-0!">
            <ScrollArea className="h-full p-4">
            <h3 className="text-base font-medium">{row?.name ?? "No skill selected"}</h3>
            {row && (
              <pre className="mt-2 text-xs whitespace-pre-wrap text-muted-foreground">
                {[
                  `Tier: ${row.tier}${row.installed ? " · installed" : " · not installed"}`,
                  `Summary: ${row.description || "—"}`,
                  `Proposal: ${row.proposal_status ?? "—"}`,
                  `Usage: ${detail ? `${detail.usage_gets} get · ${detail.usage_search_hits} search · ${detail.usage_runs} run` : "—"}`,
                  `Evidence: ${detail?.evidence_count ?? row.proposal?.source_evidence_ids.length ?? 0}`,
                ].join("\n")}
              </pre>
            )}
            {(detail?.body || row?.proposal?.body_md) && (
              <div className="mt-3 min-h-0 overflow-auto rounded-md border p-3 text-sm" dangerouslySetInnerHTML={{ __html: renderMarkdown(detail?.body || row?.proposal?.body_md || "") }} />
            )}
            {row?.proposal?.status === "pending" && (
              <div className="mt-4 flex gap-2">
                <Button size="sm" disabled={actionBusy} onClick={() => void decide(true)}>Approve</Button>
                <Button size="sm" variant="destructive" disabled={actionBusy} onClick={() => void decide(false)}>Reject</Button>
              </div>
            )}
            </ScrollArea>
          </Card>
        </TabsContent>

        <TabsContent value="wiki" className="grid min-h-0 grid-cols-[320px_minmax(0,1fr)] gap-3">
          <Card className="min-h-0 p-0!">
            <ScrollArea className="h-full p-1.5">
            {knowledge.map((node) => {
              const depth = node.path.split("/").filter(Boolean).length - 1;
              return (
                <button
                  key={node.id}
                  type="button"
                  onClick={() => setSelectedKnowledge(node.id)}
                  className={cn("mb-1 w-full rounded-md px-2 py-2 text-left hover:bg-accent", selectedKnowledge === node.id && "bg-accent")}
                  style={{ paddingLeft: 8 + depth * 12 }}
                >
                  <span className="block truncate text-sm">{node.title}</span>
                  <span className="block truncate text-xs text-muted-foreground">{node.path}</span>
                </button>
              );
            })}
            </ScrollArea>
          </Card>
          <Card className="min-h-0 p-0!">
            <ScrollArea className="h-full p-4">
            {knowledgeNode ? (
              <>
                <h3 className="text-base font-medium">{knowledgeNode.title}</h3>
                <pre className="mt-2 text-xs whitespace-pre-wrap text-muted-foreground">
                  {`${knowledgeNode.path}\n${knowledgeNode.structural ? "section" : "article"} · ${knowledgeNode.status} · confidence ${knowledgeNode.confidence.toFixed(2)}\ncreated ${formatUnixTime(knowledgeNode.created_at)} · updated ${formatUnixTime(knowledgeNode.updated_at)}`}
                </pre>
                <Separator className="my-3" />
                <div className="text-sm" dangerouslySetInnerHTML={{ __html: renderMarkdown(knowledgeNode.body_md || "_empty body_") }} />
              </>
            ) : <div className="text-sm text-muted-foreground">No node selected</div>}
            </ScrollArea>
          </Card>
        </TabsContent>

        <TabsContent value="evidence" className="grid min-h-0 grid-cols-[320px_minmax(0,1fr)] gap-3">
          <Card className="min-h-0 p-0!">
            <ScrollArea className="h-full p-1.5">
            {evidence.map((item) => (
              <button
                key={item.id}
                type="button"
                onClick={() => setSelectedEvidence(item.id)}
                className={cn("mb-1 w-full rounded-md px-2 py-2 text-left hover:bg-accent", selectedEvidence === item.id && "bg-accent")}
              >
                <span className="line-clamp-2 block text-sm">{item.summary}</span>
                <span className="block truncate text-xs text-muted-foreground">
                  {item.status} · {item.evidence_kind} · {formatUnixTime(item.created_at)}
                </span>
              </button>
            ))}
            </ScrollArea>
          </Card>
          <Card className="min-h-0 p-0!">
            <ScrollArea className="h-full p-4">
            {evidenceItem ? (
              <>
                <h3 className="text-base font-medium">{evidenceItem.summary}</h3>
                <pre className="mt-2 text-xs whitespace-pre-wrap text-muted-foreground">
                  {[
                    `ID: ${evidenceItem.id}`,
                    `Kind: ${evidenceItem.evidence_kind}`,
                    `Source: ${evidenceItem.source_kind} · ${evidenceItem.source_id}`,
                    `Seen: ${formatUnixTime(evidenceItem.last_seen_at)} ×${evidenceItem.occurrence_count}`,
                    evidenceItem.anchor_path ? `Anchor: ${evidenceItem.anchor_path}` : "",
                    evidenceItem.error ? `Error: ${evidenceItem.error}` : "",
                  ].filter(Boolean).join("\n")}
                </pre>
                <Separator className="my-3" />
                <pre className="text-sm whitespace-pre-wrap">{evidenceItem.body || "(empty body)"}</pre>
              </>
            ) : <div className="text-sm text-muted-foreground">No evidence selected</div>}
            </ScrollArea>
          </Card>
        </TabsContent>
      </Tabs>
    </div>
  );
}
