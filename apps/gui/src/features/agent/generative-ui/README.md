# CopilotKit interactive drawings

Adapted from [CopilotKit/OpenIntelligentUI](https://github.com/CopilotKit/OpenIntelligentUI)
at commit `8391a94c1c3939c679e40ddd7bfc08a9ef690e70` (MIT; see LICENSE).
The imported frame composition, partial HTML processor, Websandbox loader,
Idiomorph bundle, design system and trip animator come from its
`apps/app/src/components/generative-ui` and `packages/design-system/src`.
The renderer lifecycle is adapted to Autonomics React/Vite and the gateway stream.

## Transport

The agent emits a `generative-ui` JSON fence containing the upstream channels in
order: initialHeight, placeholderMessages, css, html (string array), jsFunctions,
jsExpressions (string array). Comark parses unfinished fences and surrounding
Markdown. partial-json tracks complete CSS, HTML arrays, function strings, and
individual expression strings. Only complete script strings execute.

This is an adapter for the existing Rust agent's text stream, not an installation
of the CopilotKit Python agent, Next.js runtime, AG-UI transport or Jev router.
There is no new model provider, API key requirement, or host action bridge.
Visual explanations use the CopilotKit sandbox route. Other code fences render
as ordinary Markdown code blocks.

## Rendering

Once CSS is complete, HTML previews appear and morph with Idiomorph in an isolated
Websandbox iframe. Preview HTML is sanitized and scripts do not execute. When the
HTML array closes, the final sandbox is created. Functions install once; complete
expressions execute sequentially, once each. Async initialization must catch its
own failures; the sandbox also reports script errors and rejected promises.
Theme variables follow the GUI live without rebuilding the iframe or resetting
controls. Canvas authors can listen for `themechange`. Heights are measured and
bounded. Iframes, observers and event listeners are destroyed on replacement/unmount.

SVG/Canvas work locally. The upstream import map exposes Three.js, GSAP, D3 and
Chart.js via esm.sh; these libraries require network access and their own loading
and error states. Sandbox code cannot access the parent DOM/cookies/storage. No
sendPrompt/ToAssistant or backend action is exposed. Avoid script/event-handler
attributes in HTML; use the ordered JavaScript channels.

Run `npm run test:intelligent-ui` for the production-browser regression test.
`npm run build:gateway` builds and copies assets; rebuild/restart the gateway to
update its compile-time embedded assets.
