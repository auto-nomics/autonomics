import { useEffect, useRef, useState } from "react";
import {
  BookOpen,
  ExternalLink,
  FileText,
  NotebookPen,
  Plus,
  Trash2,
  Upload,
  X,
} from "lucide-react";

import {
  AlertDialog,
  AlertDialogAction,
  AlertDialogCancel,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogTitle,
  AlertDialogTrigger,
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
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { ScrollArea } from "@/components/ui/scroll-area";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import { Separator } from "@/components/ui/separator";
import { PdfViewer } from "@/components/bibliography/pdf-viewer";
import { Skeleton } from "@/components/ui/skeleton";
import {
  articleIdentifier,
  authorLine,
  type ArticleDetail,
  type Collection,
} from "@/lib/bib-api";

const noteTypes = ["note", "highlight", "comment"];

interface ArticleDetailPanelProps {
  detail: ArticleDetail | null;
  collections: Collection[];
  onUpload: (file: File) => void;
  onAddNote: (kind: string, content: string) => void;
  onAddToCollection: (collectionId: string) => void;
  onRemoveFromCollection: (collectionId: string, articleId: string) => void;
  onDelete: () => void;
}

export function ArticleDetailPanel({
  detail,
  collections,
  onUpload,
  onAddNote,
  onAddToCollection,
  onRemoveFromCollection,
  onDelete,
}: ArticleDetailPanelProps) {
  const [note, setNote] = useState("");
  const [noteType, setNoteType] = useState("note");
  const uploadInputRef = useRef<HTMLInputElement>(null);
  const [collectionId, setCollectionId] = useState("");
  const [showPdf, setShowPdf] = useState(false);

  useEffect(() => {
    setCollectionId("");
    setShowPdf(false);
  }, [detail?.article.id]);

  if (!detail) {
    return (
      <Card className="flex h-full min-h-0 flex-col">
        <CardContent className="flex flex-1 flex-col items-center justify-center gap-3 text-center text-muted-foreground">
          <BookOpen className="size-8 text-muted-foreground" />
          <p className="text-sm text-muted-foreground">Select a record to inspect</p>
        </CardContent>
      </Card>
    );
  }

  const article = detail.article;
  const memberships = collections.filter((collection) =>
    collection.article_ids.includes(article.id),
  );
  const availableCollections = collections.filter(
    (collection) => !collection.article_ids.includes(article.id),
  );
  const metadata = [
    { label: "DOI", value: articleIdentifier(article, ["doi"])?.value },
    { label: "PMID", value: articleIdentifier(article, ["pmid"])?.value },
    { label: "Journal", value: article.journal },
    {
      label: "Volume",
      value: article.volume
        ? `${article.volume}${article.issue ? `(${article.issue})` : ""}`
        : undefined,
    },
    { label: "Pages", value: article.pages },
    { label: "Record", value: article.id },
  ];

  return (
    <Card className="flex h-full min-h-0 flex-col">
      <CardHeader>
        <div className="flex flex-wrap items-center gap-2">
          <Badge variant="secondary">{article.source ?? "record"}</Badge>
          <Badge variant={detail.fulltext ? "default" : "outline"}>
            {detail.fulltext ? `${detail.fulltext.file_format} full text` : "metadata only"}
          </Badge>
        </div>
        <CardTitle className="mt-2 text-xl leading-tight">{article.title}</CardTitle>
        <CardDescription>
          {authorLine(article)} · {article.year ?? "n.d."}
        </CardDescription>
      </CardHeader>

      <CardContent className="flex-1 space-y-5 overflow-auto">
        <div className="grid gap-3 sm:grid-cols-2 xl:grid-cols-4">
          {metadata.map((item) => (
            <div key={item.label} className="rounded-md bg-muted/50 p-3">
              <p className="text-xs text-muted-foreground">{item.label}</p>
              <p className="mt-1 break-words text-sm">{item.value ?? "-"}</p>
            </div>
          ))}
        </div>

        {article.abstract_text && (
          <section>
            <h2 className="mb-2 flex items-center gap-2 text-sm font-medium">
              <FileText className="size-4" /> Abstract
            </h2>
            <ScrollArea className="h-48 rounded-md border p-4">
              <p className="text-sm leading-6">{article.abstract_text}</p>
            </ScrollArea>
          </section>
        )}

        <section>
          <div className="flex flex-wrap items-center justify-between gap-2">
            <h2 className="flex items-center gap-2 text-sm font-medium">
              <Upload className="size-4" /> Full text
            </h2>
            <div className="flex flex-wrap items-center gap-2">
              {detail.fulltext?.file_path.startsWith("vfs://") && (
                <Button asChild variant="outline" size="sm">
                  <a
                    href={`/api/v1/bib/articles/${encodeURIComponent(article.id)}/fulltext/raw`}
                    target="_blank"
                    rel="noreferrer"
                  >
                    <ExternalLink /> Original
                  </a>
                </Button>
              )}
              {detail.fulltext?.file_format === "pdf" && detail.fulltext.file_path.startsWith("vfs://") && (
                <Button
                  size="sm"
                  variant={showPdf ? "secondary" : "outline"}
                  onClick={() => setShowPdf((value) => !value)}
                >
                  <BookOpen /> {showPdf ? "Hide reader" : "Read PDF"}
                </Button>
              )}
              <Button size="sm" onClick={() => uploadInputRef.current?.click()}>
                <Upload /> Upload
              </Button>
            </div>
            <input
              ref={uploadInputRef}
              type="file"
              accept=".pdf,.html,.htm,.txt"
              className="hidden"
              onChange={(event) => {
                const file = event.target.files?.[0];
                if (file) onUpload(file);
                event.target.value = "";
              }}
            />
          </div>
          {detail.fulltext?.text_content && (
            <ScrollArea className="mt-3 h-64 rounded-md border p-4">
              <p className="whitespace-pre-wrap text-sm leading-6">
                {detail.fulltext.text_content}
              </p>
            </ScrollArea>
          )}
          {showPdf && detail.fulltext?.file_format === "pdf" && detail.fulltext.file_path.startsWith("vfs://") && (
            <div className="mt-3">
              <PdfViewer
                src={`/api/v1/bib/articles/${encodeURIComponent(article.id)}/fulltext/raw`}
                title={article.title}
              />
            </div>
          )}
        </section>

        <Separator />

        <section className="space-y-3">
          <h2 className="flex items-center gap-2 text-sm font-medium">
            <NotebookPen className="size-4" /> Notes
          </h2>
          <form
            className="grid gap-2 sm:grid-cols-[128px_minmax(0,1fr)_auto]"
            onSubmit={(event) => {
              event.preventDefault();
              if (!note.trim()) return;
              onAddNote(noteType, note.trim());
              setNote("");
            }}
          >
            <Select value={noteType} onValueChange={setNoteType}>
              <SelectTrigger aria-label="Note type">
                <SelectValue />
              </SelectTrigger>
              <SelectContent>
                {noteTypes.map((type) => (
                  <SelectItem key={type} value={type}>
                    {type}
                  </SelectItem>
                ))}
              </SelectContent>
            </Select>
            <Input
              value={note}
              onChange={(event) => setNote(event.target.value)}
              placeholder="Note"
            />
            <Button type="submit" size="sm">
              <Plus /> Add
            </Button>
          </form>

          <div className="space-y-2">
            {detail.annotations.map((annotation) => (
              <div key={annotation.id} className="rounded-md border p-3">
                <p className="text-sm">{annotation.content}</p>
                <p className="mt-1 text-xs text-muted-foreground">
                  {annotation.kind}
                  {annotation.page ? ` · page ${annotation.page}` : ""}
                </p>
              </div>
            ))}
          </div>
        </section>

        <Separator />

        <section className="space-y-3">
          <h2 className="text-sm font-medium">Collections</h2>
          <div className="flex flex-wrap gap-2">
            {memberships.map((collection) => (
              <Badge key={collection.id} variant="secondary" className="gap-1 py-1 pl-2 pr-1">
                {collection.name}
                <button
                  type="button"
                  className="rounded-sm p-0.5 hover:bg-muted-foreground/15"
                  aria-label={`Remove from ${collection.name}`}
                  onClick={() => onRemoveFromCollection(collection.id, article.id)}
                >
                  <X className="size-3" />
                </button>
              </Badge>
            ))}
            {!memberships.length && (
              <p className="text-sm text-muted-foreground">Unfiled</p>
            )}
          </div>

          {availableCollections.length > 0 && (
            <div className="flex flex-wrap items-center gap-2">
              <Select value={collectionId || undefined} onValueChange={setCollectionId}>
                <SelectTrigger className="w-full sm:w-72">
                  <SelectValue placeholder="Add to collection" />
                </SelectTrigger>
                <SelectContent>
                  {availableCollections.map((collection) => (
                    <SelectItem key={collection.id} value={collection.id}>
                      {collection.name}
                    </SelectItem>
                  ))}
                </SelectContent>
              </Select>
              <Button
                variant="outline"
                size="sm"
                disabled={!collectionId}
                onClick={() => {
                  if (collectionId) {
                    onAddToCollection(collectionId);
                    setCollectionId("");
                  }
                }}
              >
                <Plus /> Add
              </Button>
            </div>
          )}
        </section>

        <Separator />

        <AlertDialog>
          <AlertDialogTrigger asChild>
            <Button variant="destructive" size="sm">
              <Trash2 /> Delete article
            </Button>
          </AlertDialogTrigger>
          <AlertDialogContent>
            <AlertDialogHeader>
              <AlertDialogTitle>Delete this article?</AlertDialogTitle>
              <AlertDialogDescription>{article.title}</AlertDialogDescription>
            </AlertDialogHeader>
            <AlertDialogFooter>
              <AlertDialogCancel>Cancel</AlertDialogCancel>
              <AlertDialogAction onClick={onDelete}>Delete</AlertDialogAction>
            </AlertDialogFooter>
          </AlertDialogContent>
        </AlertDialog>
      </CardContent>
    </Card>
  );
}
