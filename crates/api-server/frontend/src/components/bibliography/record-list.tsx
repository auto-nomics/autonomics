import { Import, Library, Loader2, Search } from "lucide-react";

import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import {
  Card,
  CardContent,
  CardDescription,
  CardHeader,
  CardTitle,
} from "@/components/ui/card";
import { Input } from "@/components/ui/input";
import { ScrollArea } from "@/components/ui/scroll-area";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import { Skeleton } from "@/components/ui/skeleton";
import type { CollectionView } from "@/components/bibliography/collection-tree";
import {
  authorLine,
  identifierLabel,
  type Article,
  type Collection,
} from "@/lib/bib-api";

const identifierTypes = [
  { value: "doi", label: "DOI" },
  { value: "pmid", label: "PMID" },
  { value: "arxiv", label: "arXiv" },
  { value: "pmc", label: "PMC" },
  { value: "openalex", label: "OpenAlex" },
  { value: "s2", label: "S2" },
];

interface RecordListProps {
  activeView: CollectionView;
  collections: Collection[];
  records: Article[];
  loading: boolean;
  query: string;
  setQuery: (value: string) => void;
  onSearch: () => void;
  selectedId?: string;
  onSelect: (id: string) => void;
  importType: string;
  setImportType: (value: string) => void;
  importId: string;
  setImportId: (value: string) => void;
  importing: boolean;
  onImportId: () => void;
}

export function RecordList({
  activeView,
  collections,
  records,
  loading,
  query,
  setQuery,
  onSearch,
  selectedId,
  onSelect,
  importType,
  setImportType,
  importId,
  setImportId,
  importing,
  onImportId,
}: RecordListProps) {
  const activeCollection = activeView.startsWith("col:")
    ? collections.find((item) => `col:${item.id}` === activeView)
    : undefined;
  const viewTitle = activeCollection?.name ?? (activeView === "unfiled" ? "Unfiled Records" : "All Records");
  const memberships = new Map(
    records.map((article) => [
      article.id,
      collections.filter((collection) => collection.article_ids.includes(article.id)),
    ]),
  );

  function ArticleButton({ article, snippet }: { article: Article; snippet?: string }) {
    const articleCollections = memberships.get(article.id) ?? [];
    return (
      <Button
        variant={selectedId === article.id ? "secondary" : "ghost"}
        className="flex h-auto w-full flex-col items-start gap-2 px-3 py-3 text-left"
        onClick={() => onSelect(article.id)}
      >
        <span className="line-clamp-2 whitespace-normal break-words text-sm font-medium leading-tight">
          {article.title}
        </span>
        <span className="whitespace-normal break-words text-xs text-muted-foreground">
          {authorLine(article)} · {article.year ?? "n.d."}
        </span>
        <span className="w-full whitespace-normal break-words font-mono text-xs text-muted-foreground">
          {identifierLabel(article)}
        </span>
        {articleCollections.length > 0 && (
          <span className="flex flex-wrap gap-1">
            {articleCollections.map((collection) => (
              <Badge key={collection.id} variant="outline" className="px-1.5 py-0 text-[10px]">
                {collection.name}
              </Badge>
            ))}
          </span>
        )}
        {snippet && (
          <span className="line-clamp-2 whitespace-normal break-words text-xs text-muted-foreground">
            {snippet}
          </span>
        )}
      </Button>
    );
  }

  const recordGroups =
    activeView === "all"
      ? collections
          .map((collection) => ({
            id: collection.id,
            label: collection.name,
            articles: records.filter((article) => collection.article_ids.includes(article.id)),
          }))
          .filter((group) => group.articles.length > 0)
          .concat([
            {
              id: "__unfiled",
              label: "Unfiled",
              articles: records.filter((article) => (memberships.get(article.id) ?? []).length === 0),
            },
          ])
          .filter((group) => group.articles.length > 0)
      : activeView === "unfiled"
        ? [
            {
              id: "__unfiled",
              label: "Unfiled",
              articles: records,
            },
          ]
        : (() => {
            const collectionId = activeView.slice("col:".length);
            return [
              {
                id: collectionId,
                label:
                  collections.find((collection) => collection.id === collectionId)?.name ??
                  "Collection",
                articles: records,
              },
            ];
          })();

  return (
    <Card className="flex h-full min-h-0 flex-col overflow-hidden gap-0">
      <CardHeader className="shrink-0">
        <div className="flex flex-wrap items-center justify-between gap-2">
          <div className="min-w-0">
            <CardTitle className="truncate">{viewTitle}</CardTitle>
            <CardDescription>
              {loading
                ? "Loading records"
                : `${records.length} records${query ? ` matching "${query}"` : ""}`}
            </CardDescription>
          </div>
          <Badge variant="secondary" className="gap-1">
            <Library className="size-3" />
            {activeView === "all" ? "Library" : activeView === "unfiled" ? "Virtual collection" : "Collection"}
          </Badge>
        </div>
      </CardHeader>

      <CardContent className="shrink-0 space-y-3">
        <form
          className="flex gap-2"
          onSubmit={(event) => {
            event.preventDefault();
            onSearch();
          }}
        >
          <Input
            value={query}
            onChange={(event) => setQuery(event.target.value)}
            placeholder="Search the current collection"
          />
          <Button type="submit" size="icon" aria-label="Search records">
            {loading ? <Loader2 className="animate-spin" /> : <Search />}
          </Button>
        </form>

        <div className="grid grid-cols-[112px_minmax(0,1fr)_auto] items-center gap-2">
          <Select value={importType} onValueChange={setImportType}>
            <SelectTrigger aria-label="Identifier type">
              <SelectValue />
            </SelectTrigger>
            <SelectContent>
              {identifierTypes.map((type) => (
                <SelectItem key={type.value} value={type.value}>
                  {type.label}
                </SelectItem>
              ))}
            </SelectContent>
          </Select>
          <Input
            value={importId}
            onChange={(event) => setImportId(event.target.value)}
            placeholder="Identifier"
          />
          <Button size="sm" disabled={importing || !importId.trim()} onClick={onImportId}>
            {importing ? <Loader2 className="animate-spin" /> : <Import />} Import
          </Button>
        </div>
      </CardContent>

      <ScrollArea className="min-h-0 flex-1">
        <div className="space-y-4 px-5 pb-5">
          {loading ? (
            Array.from({ length: 7 }).map((_, index) => (
              <Skeleton key={index} className="h-24 w-full" />
            ))
          ) : recordGroups.length === 0 ? (
            <p className="text-sm text-muted-foreground">
              {activeView === "unfiled" ? "No unfiled records" : "No records in this collection"}
            </p>
          ) : (
            recordGroups.map((group) => (
              <section key={group.id} className="space-y-2">
                <h3 className="flex items-center justify-between text-xs font-medium uppercase tracking-wide text-muted-foreground">
                  <span>{group.label}</span>
                  <span>{group.articles.length}</span>
                </h3>
                <div className="space-y-2">
                  {group.articles.map((article) => (
                    <ArticleButton key={article.id} article={article} snippet={article.abstract_text ?? ""} />
                  ))}
                </div>
              </section>
            ))
          )}
        </div>
      </ScrollArea>
    </Card>
  );
}
