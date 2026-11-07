/** Deck REST client over gatewayRequest — mirrors `/api/v1/deck`. */
import { gatewayRequest } from "@/gateway";

export interface AspectRatio {
  width: number;
  height: number;
}

export interface DeckMeta {
  id: string;
  title: string;
  aspectRatio: AspectRatio | null;
  canvasWidth: number | null;
  colorSchema: "light" | "dark" | "auto" | null;
  transition: string | null;
  exportFilename: string | null;
  defaults: SlideFrontmatter | null;
  features: { katex: boolean; mermaid: boolean };
  revision: string;
  slideCount: number;
  createdAt: string;
  updatedAt: string;
}

export interface SlideFrontmatter {
  layout?: string | null;
  class?: string | null;
  clicks?: number | null;
  clicksStart?: number | null;
  hide?: boolean | null;
  hideInToc?: boolean | null;
  title?: string | null;
  level?: number | null;
  routeAlias?: string | null;
  zoom?: number | null;
  image?: string | null;
  background?: string | null;
  backgroundImage?: string | null;
  panel1?: string | null;
  panel2?: string | null;
  panel3?: string | null;
  panel4?: string | null;
  tableCaption?: string | null;
  [key: string]: unknown;
}

export interface ElementClickState {
  order: number;
  start: number;
  end: number | null;
  max: number;
  delta: number;
}

export interface ClickPlan {
  total: number;
  clicksStart: number;
  elements: ElementClickState[];
}

export interface SlideSpec {
  index: number;
  raw: string;
  frontmatter: SlideFrontmatter;
  content: string;
  note: string | null;
  title: string | null;
  level: number | null;
  images: string[];
  clickPlan: ClickPlan | null;
  revision: string;
}

export interface AssetMeta {
  assetId: string;
  filename?: string;
  mime: string;
  size: number;
  caption?: string | null;
  provenance?: string | null;
  ref: string;
}

export interface DeckPayload {
  deck: DeckMeta;
  slides: SlideSpec[];
  assets: AssetMeta[];
}

export interface DeckSummary {
  id: string;
  title: string;
  slideCount: number;
  version: number;
  createdAt: string;
  updatedAt: string;
}

function get<T>(path: string): Promise<T> {
  return gatewayRequest("GET", path) as Promise<T>;
}

export async function fetchDeck(id: string): Promise<DeckPayload> {
  return await get<DeckPayload>(`/deck/decks/${encodeURIComponent(id)}`);
}

export async function fetchDeckList(): Promise<DeckSummary[]> {
  const payload = await get<{ decks: DeckSummary[] }>("/deck/decks");
  return payload.decks ?? [];
}

/** Resolve an `assets://{id}` ref to the raw asset URL. */
export function assetUrl(deckId: string, assetId: string): string {
  return `/api/v1/deck/decks/${encodeURIComponent(deckId)}/assets/${encodeURIComponent(assetId)}/raw`;
}

/** Map `assets://{id}` refs inside markdown/HTML to raw URLs. */
export function resolveAssetRefs(deckId: string, html: string, assets: AssetMeta[]): string {
  const byId = new Map(assets.map(a => [a.assetId, a]));
  return html.replace(/assets:\/\/([A-Za-z0-9_-]+)/g, (full, id) => {
    const asset = byId.get(id);
    if (!asset) return full;
    return assetUrl(deckId, id);
  });
}
