import { cn } from "@/lib/utils";

/** Structured payload rendering for tool calls and results: field rows for
 *  objects, indexed rows for arrays, typed leaves otherwise; raw text when
 *  the payload is not JSON at all. */

/** Parse `text` as a JSON object or array; anything else stays raw text. */
export function parseMaybeJson(text: string): object | undefined {
  const trimmed = text.trim();
  if (!trimmed.startsWith("{") && !trimmed.startsWith("[")) return undefined;
  try {
    const parsed: unknown = JSON.parse(trimmed);
    return typeof parsed === "object" && parsed !== null ? parsed : undefined;
  } catch {
    return undefined;
  }
}

function JsonLeaf({ value }: { value: unknown }) {
  if (typeof value === "boolean" || value === null) {
    return <span className="italic text-muted-foreground">{String(value)}</span>;
  }
  if (typeof value === "number") {
    return <span className="tabular-nums">{String(value)}</span>;
  }
  if (value === "") {
    return <span className="text-muted-foreground/60">&quot;&quot;</span>;
  }
  return <span className="whitespace-pre-wrap">{String(value)}</span>;
}

function JsonValue({ value, depth }: { value: unknown; depth: number }) {
  if (typeof value !== "object" || value === null) {
    return <JsonLeaf value={value} />;
  }
  const entries: Array<[string, unknown]> = Array.isArray(value)
    ? value.map((item, index) => [String(index), item])
    : Object.entries(value);
  if (!entries.length) {
    return <span className="text-muted-foreground">{Array.isArray(value) ? "[]" : "{}"}</span>;
  }
  return (
    <div className={cn("flex flex-col gap-0.5", depth > 0 && "border-l pl-2")}>
      {entries.map(([key, item]) => (
        <div key={key} className="flex min-w-0 gap-2">
          {Array.isArray(value) ? (
            <span className="w-5 shrink-0 text-right text-muted-foreground/60 tabular-nums">{Number(key) + 1}</span>
          ) : (
            <span className="w-28 shrink-0 truncate text-muted-foreground" title={key}>{key}</span>
          )}
          <span className="min-w-0 flex-1">
            <JsonValue value={item} depth={depth + 1} />
          </span>
        </div>
      ))}
    </div>
  );
}

/** Render a parsed JSON payload; `fallback` shows as pre-wrapped text when
 *  the payload did not parse. */
export function ToolPayloadView({ value, fallback }: { value: object | undefined; fallback: string }) {
  if (value === undefined) {
    return <div className="whitespace-pre-wrap">{fallback}</div>;
  }
  return <JsonValue value={value} depth={0} />;
}
