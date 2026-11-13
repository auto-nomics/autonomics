import { Button } from "@/components/ui/button";
export type CaptionPathReview = {
  checked_count: number;
  skipped_count: number;
  warning_omitted_count: number;
  warnings: { arrow_id: string; label_id: string; covered_fraction: number; uncovered_path_length: number }[];
};
export function CanvasCaptionPathReview({ review, loading = false, onFocus }: { review?: CaptionPathReview; loading?: boolean; onFocus: (id: string) => void }) {
  if (loading) return <p className="shrink-0 border-b p-2 text-xs text-muted-foreground">正在检查连线说明…</p>;
  if (!review) return <p className="shrink-0 border-b p-2 text-xs text-muted-foreground">连线说明检查暂不可用，请结合预览核对。</p>;
  if (!review.checked_count && !review.skipped_count) return null;
  return <details className="shrink-0 border-b text-xs" aria-label="连线说明覆盖检查">
    <summary className="cursor-pointer p-2">连线说明拥挤提示 {review.warnings.length + review.warning_omitted_count} 处</summary>
    <div className="max-h-40 space-y-2 overflow-auto p-2">
      <p>已检查 {review.checked_count} 组，跳过 {review.skipped_count} 组，另有 {review.warning_omitted_count} 条提示未列出。文字框可能遮住大部分连线；请定位核对并调整间距或走线。这是外框与路径检查，不代表实际字形遮挡或科学内容审查。</p>
      {review.warnings.map(item => <p key={`${item.arrow_id}:${item.label_id}`} className="break-all">
        说明框覆盖约 {Math.round(item.covered_fraction * 100)}% 的连线，框外路径约 {Math.round(item.uncovered_path_length)} 单位。
        <Button size="sm" variant="outline" aria-label={`定位拥挤连线 ${item.arrow_id}`} onClick={() => onFocus(item.arrow_id)}>定位连线</Button>{" "}
        <Button size="sm" variant="outline" aria-label={`定位连线说明 ${item.label_id}`} onClick={() => onFocus(item.label_id)}>定位说明</Button>
      </p>)}
    </div>
  </details>;
}
