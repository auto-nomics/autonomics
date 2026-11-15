import { createRoot } from "react-dom/client";
import { ZoteroBundleImport } from "../src/features/bibliography/zotero-bundle-import";
export function mountZoteroBundle(target: HTMLElement) {
  createRoot(target).render(<ZoteroBundleImport onImported={() => {}} />);
}
