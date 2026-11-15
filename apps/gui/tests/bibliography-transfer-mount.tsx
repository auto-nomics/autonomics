import React from "react";
import { createRoot } from "react-dom/client";
import { LibraryTransferDialog } from "../src/features/bibliography/library-transfer-dialog";
const root = createRoot(document.body);
export function mountTransfer(mode: "import" | "export") {
  root.render(<LibraryTransferDialog key={mode} mode={mode} articleIds={["a"]} onClose={() => {}} onImported={() => { (window as any).imported = true; }} />);
}
export async function mountDuplicates() {
  const { DuplicateReview } = await import("../src/features/bibliography/duplicate-review");
  root.render(<DuplicateReview onClose={() => {}} onMerged={() => { (window as any).merged = true; }} />);
}
