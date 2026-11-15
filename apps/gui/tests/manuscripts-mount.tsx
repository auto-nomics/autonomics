import React from "react";
import { createRoot } from "react-dom/client";
import { ManuscriptsWorkspace } from "../src/features/bibliography/manuscripts";
import { ContextBasketProvider } from "../src/features/context/context-basket";
export function mountManuscripts() {
  createRoot(document.body).render(
    <div style={{ height: "100vh" }}>
      <ContextBasketProvider
        scope="test-agent"
        contextLabel="Research agent"
        onAdd={() => {}}
      >
        <ManuscriptsWorkspace />
      </ContextBasketProvider>
    </div>,
  );
}
