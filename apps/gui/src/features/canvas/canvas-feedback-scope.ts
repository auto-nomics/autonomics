export type CandidateFeedbackComparison = { kind: "formal_revision"; revision: number } | { kind: "parent_candidate"; proposal_id: string; document_sha256?: string };
export function candidateFeedbackScope(focusedId: string | undefined, comparison: CandidateFeedbackComparison): string[] {
  return ["agent-feedback", focusedId ?? "", ...(comparison.kind === "parent_candidate" ? [comparison.kind, comparison.proposal_id, comparison.document_sha256 ?? ""] : [comparison.kind, String(comparison.revision)])];
}
