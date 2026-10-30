import { Loader2, Plus, Search } from "lucide-react";
import { useState } from "react";

import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import {
  Card,
  CardContent,
  CardHeader,
  CardTitle,
} from "@/components/ui/card";
import { Input } from "@/components/ui/input";
import { ScrollArea } from "@/components/ui/scroll-area";
import { Skeleton } from "@/components/ui/skeleton";
import { formatTime, type EntrySummary, type SearchHit } from "@/lib/wiki-api";

export type ListView = "browse" | "search";

interface EntryListProps {
  entries: EntrySummary[];
  loading: boolean;
  view: ListView;
  query: string;
  setQuery: (value: string) => void;
  tagFilter: string;
  setTagFilter: (value: string) => void;
  onSearch: () => void;
  onBrowse: () => void;
  selectedId?: string;
  /** Result count label, e.g. "12 entries". */
  countLabel: string;
  onCreate: () => void;
  onSelect: (item: EntrySummary | SearchHit) => void;
}

/** Left column: search / tag filter + the flat entry list. */
export function EntryList({
  entries,
  loading,
  view,
  query,
  setQuery,
  tagFilter,
  setTagFilter,
  onSearch,
  onBrowse,
  selectedId,
  countLabel,
  onCreate,
  onSelect,
}: EntryListProps) {
  const [creating, setCreating] = useState(false);

  return (
    <Card className="flex h-full min-h-0 flex-col">
      <CardHeader className="pb-3">
        <CardTitle className="flex items-center justify-between text-base">
          <span>Wiki · {countLabel}</span>
          <Button
            size="sm"
            variant="outline"
            disabled={creating}
            onClick={() => {
              setCreating(true);
              onCreate();
              window.setTimeout(() => setCreating(false), 500);
            }}
          >
            {creating ? <Loader2 className="h-4 w-4 animate-spin" /> : <Plus className="h-4 w-4" />}
            New
          </Button>
        </CardTitle>
        <div className="flex gap-2">
          <Input
            placeholder={view === "search" ? "BM25 query…" : "Filter by title contains…"}
            value={query}
            onChange={(event) => setQuery(event.target.value)}
            onKeyDown={(event) => {
              if (event.key === "Enter") onSearch();
            }}
          />
          <Button variant={view === "search" ? "default" : "outline"} onClick={onSearch}>
            <Search className="h-4 w-4" />
          </Button>
        </div>
        <div className="flex items-center gap-2">
          <Input
            className="h-8 text-xs"
            placeholder="tags, comma separated (AND)"
            value={tagFilter}
            onChange={(event) => setTagFilter(event.target.value)}
            onKeyDown={(event) => {
              if (event.key === "Enter") onBrowse();
            }}
          />
          <Button size="sm" variant="secondary" onClick={onBrowse}>
            Browse
          </Button>
        </div>
      </CardHeader>
      <CardContent className="min-h-0 flex-1">
        <ScrollArea className="h-full pr-2">
          {loading ? (
            <div className="space-y-2">
              {Array.from({ length: 6 }).map((_, index) => (
                <Skeleton key={index} className="h-12 w-full" />
              ))}
            </div>
          ) : entries.length === 0 ? (
            <p className="py-8 text-center text-sm text-muted-foreground">No entries match.</p>
          ) : (
            <ul className="space-y-1">
              {entries.map((item) => {
                const id = "entry_id" in item ? item.entry_id : item.id;
                const summary = item as EntrySummary;
                const active = id === selectedId;
                return (
                  <li key={id}>
                    <button
                      type="button"
                      className={`w-full rounded-md px-2 py-1.5 text-left transition-colors hover:bg-accent ${
                        active ? "bg-accent" : ""
                      }`}
                      onClick={() => onSelect(item)}
                    >
                      <div className="truncate text-sm font-medium">
                        {"title" in item ? item.title : id}
                      </div>
                      <div className="mt-0.5 flex items-center gap-1.5 text-xs text-muted-foreground">
                        <span>{formatTime(summary.mtime)}</span>
                        {(summary.tags ?? []).slice(0, 3).map((tag) => (
                          <Badge key={tag} variant="secondary" className="px-1 py-0 text-[10px]">
                            {tag}
                          </Badge>
                        ))}
                      </div>
                    </button>
                  </li>
                );
              })}
            </ul>
          )}
        </ScrollArea>
      </CardContent>
    </Card>
  );
}
