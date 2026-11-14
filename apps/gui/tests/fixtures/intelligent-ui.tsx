import React, { useState } from "react";
import { createRoot } from "react-dom/client";
import "../../src/app.css";
import { IntelligentResponse } from "../../src/features/agent/intelligent-response";

function Probe() {
  const [response, setResponse] = useState({ text: "Plain answer", streaming: false });
  Object.assign(window, { updateUI: setResponse });
  return <IntelligentResponse {...response} fallback={<p data-testid="fallback">{response.text}</p>} />;
}
createRoot(document.getElementById("root")!).render(<Probe />);
