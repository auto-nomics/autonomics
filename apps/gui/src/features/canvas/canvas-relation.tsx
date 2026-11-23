import { useState } from "react";
import type { ComponentProps } from "react";
import { useTranslation } from "react-i18next";
import { serializeAsJSON } from "@excalidraw/excalidraw";
import type { CanvasInset } from "./canvas-inset";
import { gatewayRequest } from "@/gateway";
import { Button } from "@/components/ui/button";
import { Dialog, DialogContent, DialogTitle } from "@/components/ui/dialog";
type Direction = "forward" | "bidirectional" | "undirected";
const directionValue = (value: unknown): Direction | "" => value === "forward" || value === "bidirectional" || value === "undirected" ? value : "";
function observed(start: unknown, end: unknown): { direction: Direction | ""; inhibitory: boolean } {
  if (start == null && end === "arrow") return { direction: "forward", inhibitory: false };
  if (start == null && end === "bar") return { direction: "forward", inhibitory: true };
  if (start === "arrow" && end === "arrow") return { direction: "bidirectional", inhibitory: false };
  if (start == null && end == null) return { direction: "undirected", inhibitory: false };
  return { direction: "", inhibitory: false };
}
type Review = { id: string; revision: number; snapshot: string; start: string; end: string; declared: Direction | ""; inhibition?: boolean; mismatch: boolean; evidence: string[]; evidenceCount: number; evidenceUnknown: boolean; expectedEvidence: unknown; editableEvidence: boolean };
export function CanvasRelation({ projectId, canvasId, disabled, getApi, getRevision, ensureSaved, isDirty, onApplied }: ComponentProps<typeof CanvasInset>) {
  const { t } = useTranslation("canvas");
  const names: Record<Direction, string> = { forward: t("relation.directionForward"), bidirectional: t("relation.directionBidirectional"), undirected: t("relation.directionUndirected") };
  const headName = (head: unknown) => head == null ? t("relation.headNone") : head === "arrow" ? t("relation.headArrow") : head === "bar" ? t("relation.headBar") : t("relation.headOther");
  const [open, setOpen] = useState(false), [busy, setBusy] = useState(false), [error, setError] = useState("");
  const [review, setReview] = useState<Review | null>(null), [direction, setDirection] = useState<Direction | "">(""), [inhibitory, setInhibitory] = useState(false);
  const [references, setReferences] = useState<string[]>([]);
  const snapshot = () => { const api = getApi(); return api ? serializeAsJSON(api.getSceneElements(), api.getAppState(), api.getFiles(), "local") : ""; };
  async function submit(operation: Record<string, unknown>, summary: string) {
    if (!review || busy) return; setBusy(true); setError("");
    try {
      if (isDirty() || getRevision() !== review.revision || snapshot() !== review.snapshot) throw new Error(t("relation.canvasChangedRecheck"));
      const saved = await gatewayRequest("POST", `/visualization/projects/${encodeURIComponent(projectId)}/figures/${encodeURIComponent(canvasId)}/operations`, { expected_revision: review.revision, summary, operations: [operation] }) as { id: string };

      setOpen(false); onApplied(saved.id);
    } catch (cause) { setError(String(cause)); } finally { setBusy(false); }
  }
  return <Dialog open={open} onOpenChange={value => { if (!busy) setOpen(value); }}>
    <Button size="sm" variant="outline" disabled={disabled || busy} onClick={async () => {
      setOpen(true); setBusy(true); setReview(null); setError("");
      try {
        if (!await ensureSaved()) throw new Error(t("common.resolveSaveError"));
        const api = getApi(), revision = getRevision();
        if (!api || revision === undefined || isDirty()) throw new Error(t("common.canvasChangedRetry"));
        const selected = api.getAppState().selectedElementIds;
        const chosen = api.getSceneElements().filter(element => !element.isDeleted && selected[element.id] === true);
        const arrows = chosen.filter(element => element.type === "arrow");
        if (arrows.length !== 1 || chosen.some(element => element.id !== arrows[0].id && !(element.type === "text" && element.containerId === arrows[0].id))) throw new Error(t("relation.selectOneRelation"));
        const arrow = arrows[0];
        if (arrow.type !== "arrow") throw new Error(t("relation.selectRelation"));
        const data = arrow.customData;
        if (arrow.locked || (data != null && (typeof data !== "object" || Array.isArray(data))) || data?.annotationKind != null || data?.inset != null) throw new Error(t("relation.lockedUnsupported"));
        const actual = observed(arrow.startArrowhead, arrow.endArrowhead), declared = directionValue(data?.direction);
        const inhibition = typeof data?.inhibitory === "boolean" ? data.inhibitory : undefined;
        const evidence = Array.isArray(data?.evidence) ? data.evidence.filter((value: unknown): value is string => typeof value === "string") : [];
        const editableEvidence = data?.evidence == null || (Array.isArray(data.evidence) && data.evidence.length === evidence.length && evidence.length <= 100 && evidence.every((value: string) => value.length <= 2000));
        setReferences(editableEvidence ? [...evidence] : []);
        setDirection(actual.direction); setInhibitory(actual.inhibitory);
        setReview({ id: arrow.id, revision, snapshot: snapshot(), start: headName(arrow.startArrowhead), end: headName(arrow.endArrowhead), declared, inhibition, mismatch: Boolean(actual.direction && ((declared && declared !== actual.direction) || (inhibition !== undefined && inhibition !== actual.inhibitory))), evidence: evidence.slice(0, 20).map((value: string) => value.length > 600 ? `${value.slice(0, 600)}${t("relation.truncatedSuffix")}` : value), expectedEvidence: data?.evidence ?? null, editableEvidence, evidenceCount: evidence.length, evidenceUnknown: data?.evidence != null && (!Array.isArray(data.evidence) || data.evidence.length !== evidence.length) });
      } catch (cause) { setError(String(cause)); } finally { setBusy(false); }
    }}>{t("relation.reviewButton")}</Button>
    <DialogContent className="max-h-[85vh] overflow-auto sm:max-w-xl"><DialogTitle>{t("relation.title")}</DialogTitle>
      {error && <p role="alert" className="text-sm text-destructive">{error}</p>}
      {review && <div className="space-y-3 text-sm">
        <p className="break-all">{t("relation.lineSummary", { id: review.id, revision: review.revision })}</p>
        <p aria-label={t("relation.actualHeadsLabel")}>{t("relation.actualHeads", { start: review.start, end: review.end })}</p>
        <p aria-label={t("relation.declaredLabel")}>{t("relation.declared", { direction: review.declared ? names[review.declared] : t("relation.notRecorded"), inhibition: review.inhibition === undefined ? t("relation.notRecorded") : review.inhibition ? t("relation.yes") : t("relation.no") })}</p>
        {review.mismatch && <p role="status">{t("relation.mismatchNotice")}</p>}
        <section aria-label={t("relation.evidenceLabel")}><p className="font-medium">{t("relation.evidenceTitle")}</p>{review.evidence.length ? review.evidence.map((entry, index) => <p key={index} className="whitespace-pre-wrap break-all">{entry}</p>) : <p>{review.evidenceUnknown ? t("relation.evidenceUnknown") : t("relation.noEvidence")}</p>}{review.evidenceCount > review.evidence.length && <p>{t("relation.evidenceCount", { total: review.evidenceCount, shown: review.evidence.length })}</p>}{review.evidenceUnknown && <p>{t("relation.structuredNote")}</p>}</section>
        <details aria-label={t("relation.editEvidenceLabel")}><summary className="cursor-pointer">{t("relation.editEvidence")}</summary>{review.editableEvidence ? <div className="mt-2 space-y-2">
          <p>{t("relation.editHint")}</p>
          {references.map((reference,index) => <div key={index} className="flex items-start gap-2"><textarea aria-label={t("relation.referenceLabel", { index: index + 1 })} className="min-h-16 flex-1 rounded border bg-background p-2" maxLength={2000} value={reference} disabled={busy} onChange={event => setReferences(current => current.map((value,i) => i === index ? event.target.value : value))} /><Button size="sm" variant="outline" disabled={busy} aria-label={t("relation.referenceDeleteLabel", { index: index + 1 })} onClick={() => setReferences(current => current.filter((_,i) => i !== index))}>{t("relation.delete")}</Button></div>)}
          <Button size="sm" variant="outline" disabled={busy || references.length >= 100} onClick={() => setReferences(current => [...current, ""])}>{t("relation.addReference")}</Button>
          {!references.length && <p role="status">{t("relation.emptyReferences")}</p>}
          <Button size="sm" disabled={busy || references.some(value => !value.trim())} onClick={() => void submit({op:"set_relation_evidence",id:review.id,expected_evidence:review.expectedEvidence,evidence:references}, t("relation.evidenceSummary"))}>{t("relation.applyEvidence")}</Button>
        </div> : <p className="mt-2">{t("relation.evidenceNotEditable")}</p>}</details>
        <label className="block">{t("relation.newDirection")}<select aria-label={t("relation.newDirection")} className="ml-2 rounded border bg-background p-1" value={direction} disabled={busy || inhibitory} onChange={event => setDirection(directionValue(event.target.value))}><option value="">{t("relation.selectDirection")}</option>{Object.entries(names).map(([value, name]) => <option key={value} value={value}>{name}</option>)}</select></label>
        <label className="flex items-center gap-2"><input aria-label={t("relation.inhibitoryLabel")} type="checkbox" checked={inhibitory} disabled={busy || (direction !== "forward" && !inhibitory)} onChange={event => setInhibitory(event.target.checked)} />{t("relation.inhibitory")}</label>
        <p className="text-xs text-muted-foreground">{t("relation.hint")}</p>
        <Button disabled={busy || !direction} onClick={() => { if (direction) void submit({op:"set_relation_direction",id:review.id,direction,inhibitory}, t("relation.directionSummary")); }}>{t("relation.apply")}</Button>
      </div>}
    </DialogContent>
  </Dialog>;
}
