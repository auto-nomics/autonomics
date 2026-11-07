/**
 * Print-surface API calls (token-authenticated siblings of the session
 * API). Lives outside deck-api.ts so the print bundle path stays free of
 * the gateway session client.
 */
import type { AssetMeta, DeckMeta, SlideSpec } from "./deck-api";

export interface PrintPayload {
  deck: DeckMeta;
  slides: SlideSpec[];
  assets: AssetMeta[];
  token: string;
}

export async function fetchPrintData(deckId: string, token: string): Promise<PrintPayload> {
  const res = await fetch(
    `/api/v1/deck/decks/${encodeURIComponent(deckId)}/print-data?token=${encodeURIComponent(token)}`,
    { credentials: "same-origin" },
  );
  if (!res.ok) {
    throw new Error(`print-data ${res.status}: ${await res.text()}`);
  }
  return (await res.json()) as PrintPayload;
}

/** Token-authenticated asset URL for print pages. */
export function printAssetUrl(deckId: string, assetId: string, token: string): string {
  return `/api/v1/deck/decks/${encodeURIComponent(deckId)}/print-assets/${encodeURIComponent(assetId)}?token=${encodeURIComponent(token)}`;
}
