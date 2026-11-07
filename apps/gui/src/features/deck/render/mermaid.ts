/**
 * Mermaid lazy loader. The module is dynamically imported at the first
 * mermaid fence; vite inlines it into the single gui.js bundle, so the
 * cost is one-time parse of an already-downloaded chunk, and decks
 * without `features.mermaid` never touch it.
 */

type MermaidApi = {
  initialize(config: Record<string, unknown>): void;
  render(id: string, text: string): Promise<{ svg: string }>;
};

let mermaidPromise: Promise<MermaidApi> | null = null;

function loadMermaid(dark: boolean): Promise<MermaidApi> {
  mermaidPromise ??= import("mermaid").then(mod => {
    const mermaid = mod.default;
    mermaid.initialize({
      startOnLoad: false,
      securityLevel: "strict",
      theme: dark ? "dark" : "default",
    });
    return mermaid as MermaidApi;
  });
  return mermaidPromise;
}

/**
 * Replace `<pre><code class="language-mermaid">…</code></pre>` blocks in
 * `root` with rendered SVGs. Errors render the source verbatim — a bad
 * diagram must not break the slide.
 */
export async function renderMermaidBlocks(root: HTMLElement, dark: boolean): Promise<void> {
  const blocks = root.querySelectorAll<HTMLElement>("code.language-mermaid");
  if (blocks.length === 0) return;
  let mermaid: MermaidApi;
  try {
    mermaid = await loadMermaid(dark);
  } catch {
    return; // mermaid failed to load — keep the code blocks
  }
  let seq = 0;
  for (const block of Array.from(blocks)) {
    const source = block.textContent ?? "";
    const holder = document.createElement("div");
    holder.className = "deck-mermaid";
    try {
      const { svg } = await mermaid.render(`deck-mermaid-${Date.now()}-${seq++}`, source);
      holder.innerHTML = svg;
    } catch {
      holder.textContent = source;
      holder.classList.add("deck-mermaid--error");
    }
    // Replace the enclosing <pre> when possible.
    const pre = block.closest("pre") ?? block;
    pre.replaceWith(holder);
  }
}
