/**
 * Deck opener bridge. Features (agent transcript links, asset panes) open
 * deck tabs without importing the workbench — the workbench registers the
 * real implementation in `editor-kinds.tsx` (wiki-state pattern).
 */

export interface DeckOpenOptions {
  title?: string;
  /** Initial slide (1-based). */
  slideNo?: number;
  forceNew?: boolean;
}

type DeckOpener = (deckId: string, options: DeckOpenOptions) => void;

let opener: DeckOpener | null = null;

export function registerDeckOpener(implementation: DeckOpener): void {
  opener = implementation;
}

export function openDeck(deckId: string, options: DeckOpenOptions = {}): void {
  opener?.(deckId, options);
}
