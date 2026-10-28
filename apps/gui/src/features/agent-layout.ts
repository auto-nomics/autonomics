import type { Layout } from "react-resizable-panels";

/** Persisted agent-view panel layout (percent per panel id), shared by the
 *  resizable desktop layout and the initial open/closed sidebar flags. */
export const AGENT_LAYOUT_STORAGE_KEY = "autonomics.agent.layout";

export function readAgentLayout(): Layout | null {
  try {
    const raw = localStorage.getItem(AGENT_LAYOUT_STORAGE_KEY);
    if (!raw) return null;
    const parsed: unknown = JSON.parse(raw);
    if (!parsed || typeof parsed !== "object") return null;
    const layout = parsed as Record<string, unknown>;
    const valid = Object.entries(layout).every(
      ([, size]) => typeof size === "number" && size >= 0 && size <= 100,
    );
    return valid ? (parsed as Layout) : null;
  } catch {
    return null;
  }
}

export function writeAgentLayout(layout: Layout) {
  try {
    localStorage.setItem(AGENT_LAYOUT_STORAGE_KEY, JSON.stringify(layout));
  } catch {
    // preference storage may be unavailable
  }
}

/** A side counts as open while its panel holds a visible share of the row. */
export function layoutSideOpen(layout: Layout | null, panelId: string): boolean {
  return (layout?.[panelId] ?? 100) > 0.5;
}
