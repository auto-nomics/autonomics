import { useState } from "react";
import { FolderOpen, Inbox, Library, Plus, Trash2 } from "lucide-react";

import {
  AlertDialog,
  AlertDialogAction,
  AlertDialogCancel,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogTitle,
} from "@/components/ui/alert-dialog";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import {
  Card,
  CardContent,
  CardDescription,
  CardHeader,
  CardTitle,
} from "@/components/ui/card";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import { Input } from "@/components/ui/input";
import { ScrollArea } from "@/components/ui/scroll-area";
import { Separator } from "@/components/ui/separator";
import type { Collection } from "@/lib/bib-api";

export type CollectionView = "all" | "unfiled" | `col:${string}`;

interface CollectionTreeProps {
  collections: Collection[];
  activeView: CollectionView;
  allCount: number;
  onSelect: (view: CollectionView) => void;
  onCreate: (name: string) => Promise<void>;
  onDelete: (id: string) => Promise<void>;
}

export function CollectionTree({
  collections,
  activeView,
  allCount,
  onSelect,
  onCreate,
  onDelete,
}: CollectionTreeProps) {
  const [creating, setCreating] = useState(false);
  const [name, setName] = useState("");
  const [deleteId, setDeleteId] = useState<string | null>(null);
  const pendingDelete = collections.find((collection) => collection.id === deleteId);
  const filedIds = new Set(collections.flatMap((collection) => collection.article_ids));
  const unfiledCount = Math.max(0, allCount - filedIds.size);

  async function submitName(event: React.FormEvent) {
    event.preventDefault();
    if (!name.trim()) return;
    await onCreate(name.trim());
    setName("");
    setCreating(false);
  }

  return (
    <Card className="flex h-full min-h-0 flex-col overflow-hidden gap-0">
      <CardHeader className="shrink-0">
        <div className="flex items-start justify-between gap-2">
          <div>
            <CardTitle>Collections</CardTitle>
            <CardDescription>{collections.length} collections</CardDescription>
          </div>
          <Button
            variant="outline"
            size="icon"
            aria-label="New collection"
            onClick={() => setCreating((current) => !current)}
          >
            <Plus />
          </Button>
        </div>
      </CardHeader>

      {creating && (
        <CardContent className="shrink-0 pb-3">
          <form className="flex gap-2" onSubmit={submitName}>
            <Input
              autoFocus
              value={name}
              onChange={(event) => setName(event.target.value)}
              placeholder="Collection name"
            />
            <Button type="submit" size="sm" disabled={!name.trim()}>
              Save
            </Button>
          </form>
        </CardContent>
      )}

      <Separator className="shrink-0" />
      <ScrollArea className="min-h-0 flex-1">
        <CardContent className="p-2">
          <nav className="space-y-1" aria-label="Collections">
            <Button
              variant={activeView === "all" ? "secondary" : "ghost"}
              className="w-full justify-between px-2"
              onClick={() => onSelect("all")}
            >
              <span className="flex min-w-0 items-center gap-2">
                <Library className="size-4 text-primary" />
                <span className="truncate">All Records</span>
              </span>
              <Badge variant="outline">{allCount}</Badge>
            </Button>

            <Button
              variant={activeView === "unfiled" ? "secondary" : "ghost"}
              className="w-full justify-between px-2"
              onClick={() => onSelect("unfiled")}
            >
              <span className="flex min-w-0 items-center gap-2">
                <Inbox className="size-4 text-muted-foreground" />
                <span className="truncate">Unfiled</span>
              </span>
              <Badge variant="outline">{unfiledCount}</Badge>
            </Button>

            <Separator className="my-2" />

            {collections.map((collection) => {
              const key = `col:${collection.id}` as const;
              return (
                <div
                  key={collection.id}
                  className="group flex items-center rounded-md transition-colors hover:bg-accent"
                >
                  <Button
                    variant={activeView === key ? "secondary" : "ghost"}
                    className="flex min-h-10 min-w-0 flex-1 items-start justify-between gap-2 px-3 py-2 group-hover:bg-transparent"
                    onClick={() => onSelect(key)}
                  >
                    <span className="flex min-w-0 flex-1 items-start gap-2">
                      <FolderOpen className="size-4 text-primary" />
                      <span className="min-w-0 flex-1 whitespace-normal break-words text-left leading-tight">{collection.name}</span>
                    </span>
                    <Badge variant="outline">{collection.article_ids.length}</Badge>
                  </Button>
                  <DropdownMenu>
                    <DropdownMenuTrigger asChild>
                      <Button
                        variant="ghost"
                        size="icon"
                        className="size-8 opacity-0 group-hover:opacity-100 focus-visible:opacity-100"
                        aria-label={`Actions for ${collection.name}`}
                      >
                        <Trash2 className="size-3.5" />
                      </Button>
                    </DropdownMenuTrigger>
                    <DropdownMenuContent align="end">
                      <DropdownMenuItem onSelect={() => setDeleteId(collection.id)}>
                        <Trash2 /> Delete collection
                      </DropdownMenuItem>
                    </DropdownMenuContent>
                  </DropdownMenu>
                </div>
              );
            })}

            {!collections.length && (
              <p className="rounded-md border border-dashed p-4 text-center text-sm text-muted-foreground">
                No collections
              </p>
            )}
          </nav>
        </CardContent>
      </ScrollArea>

      <AlertDialog open={Boolean(pendingDelete)} onOpenChange={(open) => !open && setDeleteId(null)}>
        <AlertDialogContent>
          <AlertDialogHeader>
            <AlertDialogTitle>Delete this collection?</AlertDialogTitle>
            <AlertDialogDescription>
              {pendingDelete?.name}. Articles remain in your library.
            </AlertDialogDescription>
          </AlertDialogHeader>
          <AlertDialogFooter>
            <AlertDialogCancel>Cancel</AlertDialogCancel>
            <AlertDialogAction
              onClick={() => {
                if (pendingDelete) void onDelete(pendingDelete.id);
                setDeleteId(null);
              }}
            >
              Delete
            </AlertDialogAction>
          </AlertDialogFooter>
        </AlertDialogContent>
      </AlertDialog>
    </Card>
  );
}
