/**
 * The single markdown pipeline for the whole app.
 *
 * One factory, two instances:
 *  - `renderMarkdown` — agent transcript and prose panes: GFM + KaTeX math
 *    ($…$, $$…$$, \(…\), \[…\]).
 *  - `renderWikiMarkdown` — the wiki adds inert `[[wikilink]]` chips
 *    (click routing stays in React). Math behaves identically.
 *
 * Everything renders through DOMPurify with the html + mathMl profiles.
 * KaTeX runs in MathML output mode: the gateway serves the SPA as three
 * embedded text assets, so there are no font files to host — the browser
 * renders `<math>` natively and the original TeX stays in
 * `<semantics><annotation>` (copyable).
 */
import DOMPurify from "dompurify";
import katex from "katex";
import { Marked, type Tokens } from "marked";

export function escapeHtml(value: unknown): string {
  return String(value)
    .replaceAll("&", "&amp;")
    .replaceAll("<", "&lt;")
    .replaceAll(">", "&gt;")
    .replaceAll('"', "&quot;")
    .replaceAll("'", "&#39;");
}

// ── math ──────────────────────────────────────────────────────────────

interface MathToken extends Tokens.Generic {
  tex: string;
  displayMode: boolean;
}

interface WikilinkToken extends Tokens.Generic {
  target: string;
  display: string;
}

/**
 * `$$…$$` / `\[…\]` may span lines; `$…$` / `\(…\)` are single-line and
 * must not open or close with whitespace (keeps "$5 and $10" out of the
 * math pipeline).
 */
const MATH_DISPLAY_TOKEN_RE = /^\$\$([\s\S]+?)\$\$/;
const MATH_INLINE_TOKEN_RE = /^\$([^\$\n]+)\$/;
const MATH_DISPLAY_BRACKET_RE = /^\\\[([\s\S]+?)\\\]/;
const MATH_INLINE_PAREN_RE = /^\\\(([\s\S]+?)\\\)/;
const WIKILINK_TOKEN_RE = /^\[\[([^\[\]\n]+)\]\]/;

function mathTokenizer(
  src: string,
  pattern: RegExp,
  name: string,
  displayMode: boolean,
): MathToken | undefined {
  const match = pattern.exec(src);
  if (!match) return undefined;
  const tex = match[1].trim();
  if (!tex) return undefined;
  if (!displayMode && (/^\s/.test(match[1]) || /\s$/.test(match[1]))) {
    return undefined;
  }
  // The token type must equal the extension name — marked resolves the
  // renderer through it.
  return { type: name, raw: match[0], tex, displayMode };
}

function mathRenderer(token: Tokens.Generic): string {
  const math = token as MathToken;
  const cssClass = math.displayMode ? "md-math md-math--display" : "md-math";
  const rendered = katex.renderToString(math.tex, {
    displayMode: math.displayMode,
    throwOnError: false,
    strict: false,
    // The gateway embeds the SPA as text assets — MathML needs no fonts.
    output: "mathml",
  });
  return `<span class="${cssClass}">${rendered}</span>`;
}

const mathExtensions = [
  {
    name: "math-display",
    level: "inline" as const,
    start(src: string) {
      const index = src.indexOf("$$");
      return index === -1 ? undefined : index;
    },
    tokenizer(src: string) {
      return mathTokenizer(src, MATH_DISPLAY_TOKEN_RE, "math-display", true);
    },
    renderer: mathRenderer,
  },
  {
    // LaTeX bracket display form. Extensions run before marked's escape
    // tokenizer, so `\[…\]` wins; an unclosed `\[` falls through to the
    // escape tokenizer and renders a literal `[`.
    name: "math-display-bracket",
    level: "inline" as const,
    start(src: string) {
      const index = src.indexOf("\\[");
      return index === -1 ? undefined : index;
    },
    tokenizer(src: string) {
      return mathTokenizer(src, MATH_DISPLAY_BRACKET_RE, "math-display-bracket", true);
    },
    renderer: mathRenderer,
  },
  {
    // LaTeX parenthesis inline form.
    name: "math-inline-paren",
    level: "inline" as const,
    start(src: string) {
      const index = src.indexOf("\\(");
      return index === -1 ? undefined : index;
    },
    tokenizer(src: string) {
      return mathTokenizer(src, MATH_INLINE_PAREN_RE, "math-inline-paren", false);
    },
    renderer: mathRenderer,
  },
  {
    name: "math-inline",
    level: "inline" as const,
    start(src: string) {
      const index = src.indexOf("$");
      return index === -1 ? undefined : index;
    },
    tokenizer(src: string) {
      return mathTokenizer(src, MATH_INLINE_TOKEN_RE, "math-inline", false);
    },
    renderer: mathRenderer,
  },
];

// ── wikilinks ─────────────────────────────────────────────────────────

const wikilinkExtension = {
  name: "wikilink",
  level: "inline" as const,
  start(src: string) {
    const index = src.indexOf("[[");
    return index === -1 ? undefined : index;
  },
  tokenizer(src: string): WikilinkToken | undefined {
    const match = WIKILINK_TOKEN_RE.exec(src);
    if (!match) return undefined;
    const pipe = match[1].indexOf("|");
    const target = (pipe === -1 ? match[1] : match[1].slice(0, pipe)).trim();
    const display = (pipe === -1 ? match[1] : match[1].slice(pipe + 1)).trim();
    if (!target) return undefined;
    return { type: "wikilink", raw: match[0], target, display: display || target };
  },
  renderer(token: Tokens.Generic) {
    const link = token as WikilinkToken;
    return `<a data-wikilink="${escapeHtml(link.target)}" class="md-wikilink">${escapeHtml(link.display)}</a>`;
  },
};

// ── pipeline factory ──────────────────────────────────────────────────

export interface MarkdownPipeline {
  parse(text: string): string;
  lexer(content: string): Tokens.Generic[];
}

function createMarkdownPipeline(options: { wikilinks?: boolean } = {}): MarkdownPipeline {
  const md = new Marked();
  md.use({
    gfm: true,
    breaks: true,
    extensions: [
      ...mathExtensions,
      ...(options.wikilinks ? [wikilinkExtension] : []),
    ],
    renderer: {
      // Code shell (language header + copy button) — views wire their own
      // delegated copy handler.
      code(token: Tokens.Code) {
        const language = (token.lang ?? "").trim().split(/\s+/)[0] ?? "";
        return [
          '<div class="md-code">',
          '<div class="md-code-head">',
          `<span>${escapeHtml(language)}</span>`,
          '<button type="button" data-copy-code>Copy</button>',
          "</div>",
          `<pre><code>${escapeHtml(token.text)}</code></pre>`,
          "</div>",
        ].join("");
      },
    },
  });
  return {
    parse: (text) => md.parse(text, { async: false }),
    lexer: (content) => md.lexer(content) as Tokens.Generic[],
  };
}

const SANITIZE = {
  // html for the markdown body, mathMl for KaTeX's `<math>` output.
  // KaTeX also nests the original TeX in <semantics><annotation> — not in
  // DOMPurify's mathMl profile, added back as inert text so the formula
  // source stays copyable.
  USE_PROFILES: { html: true, mathMl: true },
  ADD_TAGS: ["semantics", "annotation"],
};

/** Agent transcript and prose panes: math yes, wikilinks no. */
const agentPipeline = createMarkdownPipeline();

export function renderMarkdown(text: string): string {
  return DOMPurify.sanitize(agentPipeline.parse(text), SANITIZE);
}

/** Wiki pipeline: identical rendering plus `[[wikilink]]` chips. */
const wikiPipeline = createMarkdownPipeline({ wikilinks: true });

export function renderWikiMarkdown(text: string): string {
  return DOMPurify.sanitize(wikiPipeline.parse(text), SANITIZE);
}

/** Block-lexer access for the wiki block editor (lossless splitting). */
export function wikiLexer(content: string): Tokens.Generic[] {
  return wikiPipeline.lexer(content);
}

/**
 * Escape hatch for feature-local pipelines (deck slides add asset-ref +
 * click-directive handling on top of the same math/GFM base). The deck
 * feature sanitizes with its own profile (it injects data-* attributes).
 */
export { createMarkdownPipeline, SANITIZE };
