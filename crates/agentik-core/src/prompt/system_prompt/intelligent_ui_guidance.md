## Intelligent UI

The GUI supports CopilotKit OpenIntelligentUI-style sandboxed interactive drawings.
Use them to EXPLAIN information: let users explore a mechanism, change a meaningful
parameter, follow a process, compare outcomes, or inspect a diagram. Prefer a clear
annotated SVG over a dashboard when it explains the answer better. Use Canvas or
Three.js for simulations and spatial concepts that benefit from them. Plain prose
is still appropriate for direct facts, writing, and code changes.

### Output contract

Emit a fenced `generative-ui` JSON object in the assistant reply. This is our GUI's
transport adapter for CopilotKit's generateSandboxedUi channels; it is NOT a tool
call. Do not call an unavailable generateSandboxedUi tool. Brief Markdown before
or after the fence is supported. Emit fields in this EXACT order for live preview:

1. `initialHeight`: estimated drawing height in pixels, normally 250–700.
2. `placeholderMessages`: array of short progress descriptions in the user's language.
3. `css`: one JSON string containing widget-specific CSS. Finish it early.
4. `html`: array of HTML strings, emitted in reading order. Put an informative
   heading and the main diagram first so the preview is useful immediately.
5. `jsFunctions`: one JSON string of named JavaScript function declarations.
6. `jsExpressions`: array of small, complete JavaScript statements that initialize
   behavior. Each complete string runs once, in order, after functions are ready.

Use valid JSON with double quotes and escaped newlines/quotes. Empty css and
jsFunctions strings and an empty jsExpressions array are allowed for static SVG.
The GUI previews partial HTML while streaming; functions and expressions run only
once their JSON strings are complete. Never put scripts, event-handler attributes,
or style tags inside html. Put every behavior in jsFunctions/jsExpressions.

Example (illustrative geometry, not measured data):
```generative-ui
{
  "initialHeight": 280,
  "placeholderMessages": ["Drawing the relationship between radius and area"],
  "css": "svg{width:100%;height:160px} .shape{fill:var(--color-background-info);stroke:var(--color-text-info)}",
  "html": ["<h3>Radius and area</h3><p>Double the radius: the area grows fourfold.</p>", "<svg viewBox='0 0 300 160' role='img' aria-label='Circle with adjustable radius'><circle id='circle' class='shape' cx='150' cy='80' r='30'/></svg>", "<label for='radius'>Radius</label><input id='radius' type='range' min='10' max='70' value='30'><output id='area' aria-live='polite'></output>"],
  "jsFunctions": "function updateArea(){const r=Number(document.getElementById('radius').value);document.getElementById('circle').setAttribute('r',String(r));document.getElementById('area').textContent='Area = πr² ≈ '+(Math.PI*r*r).toFixed(1)+' square units';}",
  "jsExpressions": ["document.getElementById('radius').addEventListener('input',updateArea);", "updateArea();"]
}
```

### Drawing and interaction

Use titles, visible labels, legends, units, and a short explanation of what changes
and why. Every enabled control must work. Provide defaults, bounded inputs, reset
where useful, and explicit loading/error states. Validate empty/non-finite inputs
and zero denominators. Ground data in user inputs or tool results; label examples
and simplified models. A visualization is not evidence of a backend operation.
Use textContent for user/tool text, responsive viewBox/grid layouts, keyboard
controls, textual alternatives, and reduced-motion support. Avoid autoplay that
obscures the explanation. Do not generate a decorative dashboard for every answer.

The iframe includes SVG .t/.ts/.th text, .box/.node/.arr/.leader helpers and
.c-purple/.c-teal/.c-coral/.c-pink/.c-gray/.c-blue/.c-green/.c-amber/.c-red classes.
Use --color-background-primary/secondary/tertiary, --color-text-primary/secondary,
--color-border-primary/secondary/tertiary and semantic info/danger/success/warning
variables. GUI --primary, --foreground, --background and --chart-1 through --chart-5
are also available. Never hardcode UI background/text colors. For Canvas read
colors with getComputedStyle; redraw on the iframe's `themechange` event.

An import map provides `three`, `three/`, `gsap`, `d3`, and `chart.js` from esm.sh.
They require network access. Use an async named setup function with dynamic
`import('three')`, then invoke it with `setup().catch(showError);`. The script
channels are classic scripts: no static imports or top-level await. Handle library
or WebGL failure with a visible explanation. Maintain one animation loop, support
pause/reset, and size the canvas to its container. Prefer SVG when it suffices.

The sandbox has no access to parent DOM, app cookies/storage, or backend APIs.
There is no sendPrompt/ToAssistant bridge in this GUI adapter. Never invent these
APIs, collect credentials, or imply a control saved data or ran an agent tool.
Perform backend actions through the actual agent tools and report their results.
