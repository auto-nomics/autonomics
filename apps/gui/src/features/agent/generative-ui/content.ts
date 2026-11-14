import { parse, Allow } from "partial-json";
import { z } from "zod";

// CopilotKit's generateSandboxedUi channels, carried over our existing text stream.
const payloadSchema = z.object({
  initialHeight: z.number().finite().optional(),
  placeholderMessages: z.array(z.string()).optional(),
  css: z.string().optional(),
  html: z.array(z.string()).optional(),
  jsFunctions: z.string().optional(),
  jsExpressions: z.array(z.string()).optional(),
});
export type DrawingContent = z.infer<typeof payloadSchema> & {
  generating: boolean;
  cssComplete: boolean;
  htmlComplete: boolean;
  jsFunctionsComplete: boolean;
};

export function parseDrawingContent(source: string, streaming: boolean): DrawingContent {
  if (!source.trim() && streaming) return { generating: true, cssComplete: false, htmlComplete: false, jsFunctionsComplete: false };
  const partial = payloadSchema.parse(streaming ? parse(source, Allow.OBJ | Allow.ARR | Allow.STR) : JSON.parse(source));
  // A closed JSON string is necessary before executing JS; arrays can still grow.
  const strings = payloadSchema.parse(streaming ? parse(source, Allow.OBJ | Allow.ARR) : partial);
  // With only partial objects allowed, an html array appears only once its ] arrives.
  const fields = payloadSchema.parse(streaming ? parse(source, Allow.OBJ) : partial);
  if (!streaming && !partial.html?.length) throw new Error("绘图回复缺少 html 内容。");
  return {
    ...partial,
    jsFunctions: fields.jsFunctions,
    jsExpressions: strings.jsExpressions,
    generating: streaming,
    cssComplete: fields.css !== undefined || (!streaming && partial.css === undefined),
    htmlComplete: fields.html !== undefined,
    jsFunctionsComplete: fields.jsFunctions !== undefined || (!streaming && partial.jsFunctions === undefined),
  };
}
