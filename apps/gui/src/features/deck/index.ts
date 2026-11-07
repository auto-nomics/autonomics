/** Deck feature barrel: layers may only import through here. */
import "./deck.css";

export { fetchDeck, fetchDeckList, assetUrl, resolveAssetRefs } from "./deck-api";
export type {
  DeckMeta,
  DeckPayload,
  DeckSummary,
  SlideSpec,
  SlideFrontmatter,
  ClickPlan,
  AssetMeta,
} from "./deck-api";
export { registerDeckOpener, openDeck } from "./deck-state";
export type { DeckOpenOptions } from "./deck-state";
export { DeckEditorBody } from "./deck-editor";
export { DeckRenderer } from "./render/deck-renderer";
export { SlideView } from "./render/slide-view";
export { renderSlideContent, renderMermaidBlocks } from "./render/markdown";
