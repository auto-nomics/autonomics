import { useState } from "react";
import { useTranslation } from "react-i18next";
import { gatewayRequest } from "@/gateway";
import type { BibCollection, BibDetail } from "@/types";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { SearchDialog } from "./search-dialog";
import { collectionPath } from "./collection-tree-model";

export function BatchOrganize({ ids, collections, onClose, onChanged }: {
  ids: string[]; collections: BibCollection[]; onClose: () => void; onChanged: () => void;
}) {
  const { t } = useTranslation("bibliography");
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
  return <SearchDialog title={t("batchOrganize.title")} onClose={() => { if (!busy) onClose(); }} footer={
    <div className="flex justify-end gap-2"><Button variant="ghost" disabled={busy} onClick={onClose}>{t("batchOrganize.close")}</Button>
      <Button disabled={busy || done.length === ids.length || (operation.endsWith("tags") ? !tags.split(",").some(value => value.trim()) : !collection)} onClick={() => void run()}>{busy ? t("batchOrganize.applying") : failures.length ? t("batchOrganize.retryFailed") : t("batchOrganize.apply")}</Button></div>
  }>
    <p className="mb-3 text-sm">{t("batchOrganize.summary", { selected: ids.length, updated: done.length })}</p>
    <fieldset disabled={busy || started} className="space-y-3">
      <label className="block">{t("batchOrganize.operation")}<select aria-label={t("batchOrganize.operationAria")} className="mt-1 block w-full rounded border bg-background p-2" value={operation} onChange={event => setOperation(event.target.value)}>
        <option value="add-tags">{t("batchOrganize.opAddTags")}</option><option value="remove-tags">{t("batchOrganize.opRemoveTags")}</option><option value="add-collection">{t("batchOrganize.opAddCollection")}</option><option value="remove-collection">{t("batchOrganize.opRemoveCollection")}</option>
      </select></label>
      {operation.endsWith("tags") ? <label className="block">{t("batchOrganize.tags")}<Input aria-label={t("batchOrganize.tagsAria")} value={tags} onChange={event => setTags(event.target.value)} /></label>
        : <label className="block">{t("batchOrganize.collection")}<select aria-label={t("batchOrganize.collectionAria")} className="mt-1 block w-full rounded border bg-background p-2" value={collection} onChange={event => setCollection(event.target.value)}><option value="">{t("batchOrganize.selectCollection")}</option>{collections.filter(item => !item.deleted).map(item => <option key={item.id} value={item.id}>{collectionPath(collections, item.id)}</option>)}</select></label>}
    </fieldset>
    {failures.length > 0 && <div role="alert" className="mt-3 text-sm text-destructive"><p>{t("batchOrganize.failures", { count: failures.length })}</p><ul>{failures.map(item => <li key={item.id}>{item.id}: {item.error}</li>)}</ul></div>}
    {done.length === ids.length && <p role="status" className="mt-3 text-sm">{t("batchOrganize.allUpdated")}</p>}
  </SearchDialog>;
}
