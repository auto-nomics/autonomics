import { Puzzle } from "lucide-react";
import { Button } from "@/components/ui/button";
import { useState } from "react";
import { SearchDialog } from "./search-dialog";

export function BrowserConnectorHelp() {
  const [open, setOpen] = useState(false);
  const gateway = /^https?:$/.test(window.location.protocol) && !("__TAURI_INTERNALS__" in window)
    ? window.location.origin : "http://127.0.0.1:8765";
  return <>
    <Button size="icon-sm" variant="ghost" title="Browser import connector" aria-label="Browser import connector" onClick={() => setOpen(true)}><Puzzle /></Button>
    {open && <SearchDialog title="Import from your browser" onClose={() => setOpen(false)}>
      <div className="space-y-4 text-sm">
        <p>Save article metadata from the page you are reading, review it, and choose a collection before importing.</p>
        <ol className="list-decimal space-y-2 pl-5">
          <li>In Chrome or Edge, open the Extensions page and enable Developer mode.</li>
          <li>Choose “Load unpacked” and select <code>apps/bib-connector</code> from this installation’s source checkout, or the unpacked connector distribution supplied by your administrator.</li>
          <li>Open an article page and click the Autonomics connector icon.</li>
          <li>Enter your gateway address and token, then click Connect. Review the metadata and click Save metadata.</li>
        </ol>
        <div className="rounded border p-3"><div className="font-medium">Gateway address</div><code className="break-all">{gateway}</code><p className="mt-1 text-xs text-muted-foreground">Use the address of your running gateway. Desktop users may need a different port. Remote gateways require HTTPS.</p></div>
        <p>The token stays in the extension’s browser-session storage and is never sent to the article page. Use “Forget token” to clear it.</p>
        <p className="text-muted-foreground">The connector saves metadata and the source URL. PDF files and webpage snapshots are not downloaded automatically. Pending captures can be kept on this device and retried later.</p>
      </div>
    </SearchDialog>}
  </>;
}
