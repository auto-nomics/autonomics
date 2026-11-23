import { useTranslation } from "react-i18next";
import { Button } from "@/components/ui/button";
export type CaptionPathReview = {
  checked_count: number;
  skipped_count: number;
  warning_omitted_count: number;
  warnings: { arrow_id: string; label_id: string; covered_fraction: number; uncovered_path_length: number }[];
};
export function CanvasCaptionPathReview({ review, loading = false, onFocus }: { review?: CaptionPathReview; loading?: boolean; onFocus: (id: string) => void }) {
  const { t } = useTranslation("canvas");
  if (loading) return <p className="shrink-0 border-b p-2 text-xs text-muted-foreground">{t("captionPathReview.checking")}</p>;
  if (!review) return <p className="shrink-0 border-b p-2 text-xs text-muted-foreground">{t("captionPathReview.unavailable")}</p>;
  if (!review.checked_count && !review.skipped_count) return null;
  return <details className="shrink-0 border-b text-xs" aria-label={t("captionPathReview.sectionLabel")}>
    <summary className="cursor-pointer p-2">{t("captionPathReview.summary", { count: review.warnings.length + review.warning_omitted_count })}</summary>
    <div className="max-h-40 space-y-2 overflow-auto p-2">
      <p>{t("captionPathReview.description", { checked: review.checked_count, skipped: review.skipped_count, omitted: review.warning_omitted_count })}</p>
      {review.warnings.map(item => <p key={`${item.arrow_id}:${item.label_id}`} className="break-all">
        {t("captionPathReview.coverage", { percent: Math.round(item.covered_fraction * 100), length: Math.round(item.uncovered_path_length) })}
        <Button size="sm" variant="outline" aria-label={t("captionPathReview.locateArrowLabel", { id: item.arrow_id })} onClick={() => onFocus(item.arrow_id)}>{t("captionPathReview.locateArrow")}</Button>{" "}
        <Button size="sm" variant="outline" aria-label={t("captionPathReview.locateLabelLabel", { id: item.label_id })} onClick={() => onFocus(item.label_id)}>{t("captionPathReview.locateLabel")}</Button>
      </p>)}
    </div>
  </details>;
}
