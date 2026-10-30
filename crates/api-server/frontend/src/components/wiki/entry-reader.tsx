import ReactMarkdown from "react-markdown";
import remarkGfm from "remark-gfm";

import { Badge } from "@/components/ui/badge";
import { Card, CardContent, CardHeader } from "@/components/ui/card";
import { ScrollArea } from "@/components/ui/scroll-area";
import type { WikiEntry } from "@/lib/wiki-api";

/** Center-pane read view: rendered markdown + tag/alias chips. */
export function EntryReader({ entry }: { entry: WikiEntry }) {
  return (
    <Card className="flex h-full min-h-0 flex-col">
      <CardHeader className="pb-2">
        <h1 className="break-words text-lg font-semibold">{entry.title}</h1>
        <div className="flex flex-wrap items-center gap-1.5">
          {entry.tags.map((tag) => (
            <Badge key={tag} variant="secondary">
              {tag}
            </Badge>
          ))}
          {entry.alias.map((name) => (
            <Badge key={name} variant="outline">
              alias: {name}
            </Badge>
          ))}
        </div>
      </CardHeader>
      <CardContent className="min-h-0 flex-1">
        <ScrollArea className="h-full pr-3">
          <div className="prose prose-sm dark:prose-invert max-w-none break-words text-sm">
            <ReactMarkdown remarkPlugins={[remarkGfm]}>{entry.content}</ReactMarkdown>
          </div>
        </ScrollArea>
      </CardContent>
    </Card>
  );
}
