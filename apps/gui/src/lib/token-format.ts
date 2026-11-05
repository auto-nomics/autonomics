export type TokenUsage = { input: number; output: number; cache: number };

/** Formats token quantities consistently as compact decimal thousands. */
export function formatTokenCount(value: number): string {
  const count = Number.isFinite(value) ? Math.max(0, value) : 0;
  if (count === 0) return "0k";
  const thousands = count / 1_000;
  const digits = thousands < 100 ? 1 : 0;
  return `${thousands.toFixed(digits).replace(/\.0$/, "")}k`;
}

export function formatTokenUsage(usage: TokenUsage): string {
  const values = [`${formatTokenCount(usage.input)} in`, `${formatTokenCount(usage.output)} out`];
  if (usage.cache) values.push(`${formatTokenCount(usage.cache)} cache`);
  return values.join(" · ");
}
