import { wikiLexer } from "@/lib/markdown";

type Token = {
  type: string; raw?: string; tex?: string; tokens?: Token[]; items?: Token[];
  header?: { tokens: Token[] }[]; rows?: { tokens: Token[] }[][];
};

/** Lute uses dollar delimiters. Preserve the meaning of existing LaTeX
 * bracket notation while leaving fenced/inline code and other source alone. */
export function wikiMarkdownForVditor(markdown: string): string {
  if (!/\\[[(]/.test(markdown)) return markdown;
  const rewrite = (source: string, tokens: Token[]): string => {
    let result = "", offset = 0;
    for (const token of tokens) {
      if (!token.raw) continue;
      const from = source.indexOf(token.raw, offset);
      if (from < 0) continue;
      let value = token.raw;
      if (token.type === "math-inline-paren") value = `$${token.tex}$`;
      else if (token.type.startsWith("math-display-bracket")) value = `$$\n${token.tex}\n$$`;
      else {
        const children = token.tokens ?? token.items ?? (token.header
          ? [...token.header, ...(token.rows ?? []).flat()].flatMap(cell => cell.tokens) : []);
        if (children.length) value = rewrite(value, children);
      }
      result += source.slice(offset, from) + value;
      offset = from + token.raw.length;
    }
    return result + source.slice(offset);
  };
  return rewrite(markdown, wikiLexer(markdown) as Token[]);
}
