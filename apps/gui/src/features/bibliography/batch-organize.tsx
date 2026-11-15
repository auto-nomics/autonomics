import { useState } from "react";
import { gatewayRequest } from "@/gateway";
import type { BibCollection, BibDetail } from "@/types";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { SearchDialog } from "./search-dialog";
import { collectionPath } from "./collection-tree-model";

export function BatchOrganize({ ids, collections, onClose, onChanged }: {
  ids: string[]; collections: BibCollection[]; onClose: () => void; onChanged: () => void;
}) {
  const [operation, setOperation] = useState("add-tags");
  const [tags, setTags] = useState("");
  const [collection, setCollection] = useState("");
  const [busy, setBusy] = useState(false);
  const [done, setDone] = useState<string[]>([]);
  const [failures, setFailures] = useState<Array<{ id: string; error: string }>>([]);
  const [started, setStarted] = useState(false);
  const run = async () => {
    setBusy(true); setFailures([]); setStarted(true);
    const completed = new Set(done);
    const failed: Array<{ id: string; error: string }> = [];
    const values = [...new Set(tags.split(",").map(value => value.trim()).filter(Boolean))];
    for (const id of ids.filter(id => !completed.has(id))) {
      try {
        if (operation.endsWith("tags")) {
          const detail: BibDetail = await gatewayRequest("GET", `/bib/articles/${encodeURIComponent(id)}`);
          const current = detail.article.keywords ?? [];
          const keywords = operation === "add-tags" ? [...new Set([...current, ...values])] : current.filter(tag => !values.includes(tag));
          await gatewayRequest("PUT", `/bib/articles/${encodeURIComponent(id)}`, { ...detail.article, keywords, expected_revision: detail.revision });
        } else {
          await gatewayRequest(operation === "add-collection" ? "POST" : "DELETE",
            `/bib/collections/${encodeURIComponent(collection)}/articles${operation === "remove-collection" ? `/${encodeURIComponent(id)}` : ""}`,
            operation === "add-collection" ? { article_id: id } : undefined);
        }
        completed.add(id); setDone([...completed]);
      } catch (failure) { failed.push({ id, error: String(failure) }); }
    }
    setFailures(failed); setBusy(false); onChanged();
  };
  return <SearchDialog title="Organize selected articles" onClose={() => { if (!busy) onClose(); }} footer={
    <div className="flex justify-end gap-2"><Button variant="ghost" disabled={busy} onClick={onClose}>Close</Button>
      <Button disabled={busy || done.length === ids.length || (operation.endsWith("tags") ? !tags.split(",").some(value => value.trim()) : !collection)} onClick={() => void run()}>{busy ? "Applying…" : failures.length ? "Retry failed articles" : "Apply"}</Button></div>
  }>
    <p className="mb-3 text-sm">{ids.length} selected articles. {done.length} updated.</p>
    <fieldset disabled={busy || started} className="space-y-3">
      <label className="block">Operation<select aria-label="Batch operation" className="mt-1 block w-full rounded border bg-background p-2" value={operation} onChange={event => setOperation(event.target.value)}>
        <option value="add-tags">Add tags</option><option value="remove-tags">Remove tags</option><option value="add-collection">Add to collection</option><option value="remove-collection">Remove from collection</option>
      </select></label>
      {operation.endsWith("tags") ? <label className="block">Tags (comma separated)<Input aria-label="Batch tags" value={tags} onChange={event => setTags(event.target.value)} /></label>
        : <label className="block">Collection<select aria-label="Batch collection" className="mt-1 block w-full rounded border bg-background p-2" value={collection} onChange={event => setCollection(event.target.value)}><option value="">Select collection…</option>{collections.filter(item => !item.deleted).map(item => <option key={item.id} value={item.id}>{collectionPath(collections, item.id)}</option>)}</select></label>}
    </fieldset>
    {failures.length > 0 && <div role="alert" className="mt-3 text-sm text-destructive"><p>{failures.length} articles could not be updated. Successful changes are retained.</p><ul>{failures.map(item => <li key={item.id}>{item.id}: {item.error}</li>)}</ul></div>}
    {done.length === ids.length && <p role="status" className="mt-3 text-sm">All selected articles updated.</p>}
  </SearchDialog>;
}
