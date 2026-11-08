/**
 * Print page bootstrap (route `/deck/print?deck={id}&token={t}&export={eid}`).
 *
 * The headless exporter navigates here with no session cookie; the
 * one-time print token gates `print-data` + `print-assets`. The page
 * renders every slide full-bleed at canvas size and drives the
 * slidev-style wait protocol:
 *
 *   - `document.documentElement.dataset.deckReady = "true"` once all
 *     markdown, KaTeX and mermaid output has settled;
 *   - `[data-waitfor]` sentinels count down as async blocks finish (the
 *     exporter polls until none remain);
 *   - `window.__deckGoto(slide, click)` steps the exporter through
 *     per-slide/per-click captures.
 */
import ReactDOM from "react-dom/client";
import { fetchPrintData } from "../deck-api-print";
import { PrintDeck } from "./print-deck";

export function bootPrintPage(container: HTMLDivElement): void {
  document.documentElement.dataset.deckReady = "false";
  delete document.documentElement.dataset.deckError;
  const params = new URLSearchParams(location.search);
  const deckId = params.get("deck") ?? "";
  const token = params.get("token") ?? "";
  const clicks = params.get("clicks"); // "all" reveals every click state
  const mode = params.get("mode"); // "pdf" = stacked all-slides layout

  if (!deckId || !token) {
    container.textContent = "print page requires ?deck= and ?token=";
    document.documentElement.dataset.deckError = "print page requires ?deck= and ?token=";
    return;
  }

  fetchPrintData(deckId, token)
    .then(payload => {
      container.textContent = "";
      ReactDOM.createRoot(container).render(
        <PrintDeck
          payload={payload}
          token={token}
          revealAllClicks={clicks === "all"}
          mode={mode === "pdf" ? "pdf" : "single"}
        />,
      );
    })
    .catch((e: unknown) => {
      container.textContent = `print payload failed: ${String(e)}`;
      // Surface the real initialization failure to the exporter immediately.
      document.documentElement.dataset.deckError = String(e);
    });
}

// Satisfy the JSX runtime for this .tsx without a full React import.
void ReactDOM;
