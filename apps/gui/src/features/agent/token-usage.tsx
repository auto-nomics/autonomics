import { useTranslation } from "react-i18next";
import { cn } from "@/lib/utils";
import { formatTokenCount, type TokenUsage } from "@/lib/token-format";

type TokenUsageBadgeProps = {
  usage: TokenUsage;
  className?: string;
};

/** A compact, colour-coded summary used wherever an agent reports usage. */
export function TokenUsageBadge({ usage, className }: TokenUsageBadgeProps) {
  const { t } = useTranslation("agent");
  const title = [
    t("usage.input", { value: formatTokenCount(usage.input) }),
    t("usage.output", { value: formatTokenCount(usage.output) }),
    usage.cache ? t("usage.cache", { value: formatTokenCount(usage.cache) }) : null,
  ].filter(Boolean).join(" · ");

  return (
    <span
      className={cn("inline-flex items-center gap-1 font-medium tabular-nums", className)}
      title={title}
      aria-label={t("usage.ariaLabel", { detail: title })}
    >
      <span className="text-sky-600 dark:text-sky-400"><span aria-hidden>↓</span> {formatTokenCount(usage.input)}</span>
      <span className="text-muted-foreground/60" aria-hidden>·</span>
      <span className="text-violet-600 dark:text-violet-400"><span aria-hidden>↑</span> {formatTokenCount(usage.output)}</span>
      {usage.cache ? <>
        <span className="text-muted-foreground/60" aria-hidden>·</span>
        <span className="text-amber-600 dark:text-amber-400"><span aria-hidden>◇</span> {formatTokenCount(usage.cache)}</span>
      </> : null}
    </span>
  );
}
