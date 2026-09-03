# @jayread/pdfium-viewer

> **PDF rendering library for the JayRead project** — PDFium-based WASM engine with React adapters

A fork of [`@embedpdf/core`](https://github.com/embedpdf/embedpdf) (embedpdf project), renamed to `@jayread/pdfium-viewer` in 2026. This package provides the PDF rendering engine used throughout the JayRead project for viewing research papers and managing PDF libraries.

## Package Information

- **Name:** `@jayread/pdfium-viewer`
- **Version:** 0.1.0
- **Private:** true (JayRead workspace internal package)
- **Type:** ESM module
- **Entry:** `./src/index.ts`

## Rendering Technology

This package uses **PDFium WASM** for high-performance, client-side PDF rendering:

- **Canvas-based rendering** — All PDF pages are rendered to HTML5 Canvas elements
- **WASM engine** — PDFium compiled to WebAssembly for browser compatibility
- **Unified pipeline** — Since 2026-06-14, the native `pdfium-render` pipeline has been removed. Both web and desktop clients now use this WASM engine exclusively
- **Cross-platform** — The same rendering code runs in browsers (Chromium/Firefox/Safari) and Tauri desktop apps

### WASM Binary Loading

The PDFium WASM binary (`pdfium.wasm`) is loaded from `/wasm/pdfium.wasm` by default. The loading mechanism supports:

1. **Direct loading** — Fetch-based loading for main thread rendering
2. **Worker loading** — Web Worker-based loading for non-blocking rendering
3. **CDN fallback** — Can load from `https://cdn.jsdelivr.net/npm/@embedpdf/pdfium@__PDFIUM_VERSION__/dist/pdfium.wasm`

## History

This package was forked from the [`@embedpdf/core`](https://github.com/embedpdf/embedpdf) project in early 2026 and renamed to `@jayread/pdfium-viewer`. The fork maintains the core PDFium rendering engine while adding JayRead-specific enhancements and integrations.

**Key changes from upstream:**
- Renamed package from `@embedpdf/core` to `@jayread/pdfium-viewer`
- JayRead workspace integration (pnpm workspace dependency)
- Custom React components for research paper UI
- Enhanced annotation and selection tools for academic use

## Directory Structure

```
src/
├── core/                    # Framework-agnostic core
│   ├── base/               # Core utilities and types
│   ├── plugin/             # Plugin system foundation
│   ├── registry/           # Component registry
│   ├── shared/             # Shared utilities
│   ├── store/              # State management
│   ├── types/              # TypeScript type definitions
│   ├── utils/              # Helper functions
│   ├── react/              # React adapters for core
│   └── index.ts            # Core entry point
├── engines/                 # Rendering engines
│   ├── pdfium/             # PDFium WASM engine implementation
│   ├── react/              # React hooks for engines
│   └── index.ts
├── plugin-*/                # Feature plugins (21 plugins)
│   ├── plugin-annotation/   # Annotation tools
│   ├── plugin-bookmark/     # Bookmark navigation
│   ├── plugin-capture/      # Screenshot capture
│   ├── plugin-document-manager/ # Document management
│   ├── plugin-export/       # PDF export
│   ├── plugin-form/         # Form handling
│   ├── plugin-fullscreen/   # Fullscreen mode
│   ├── plugin-history/      # Navigation history
│   ├── plugin-interaction-manager/ # Interaction coordination
│   ├── plugin-pan/          # Pan/zoom interactions
│   ├── plugin-print/        # Printing support
│   ├── plugin-render/       # Rendering coordination
│   ├── plugin-rotate/       # Page rotation
│   ├── plugin-scroll/       # Scroll handling
│   ├── plugin-search/       # Text search
│   ├── plugin-selection/    # Text selection
│   ├── plugin-spread/       # Multi-page spreads
│   ├── plugin-thumbnail/    # Thumbnail navigation
│   ├── plugin-tiling/       # Tiled rendering
│   ├── plugin-viewport/     # Viewport management
│   └── plugin-zoom/         # Zoom controls
├── models/                   # Data models
├── fonts/                    # Font handling
├── pdfium/                   # PDFium WASM bindings
├── utils/                    # Shared utilities
├── framework.ts              # Framework initialization
└── index.ts                  # Package entry point
```

## Integration in JayRead

This package is consumed by `packages/web` as a workspace dependency:

```json
{
  "dependencies": {
    "@jayread/pdfium-viewer": "workspace:*"
  }
}
```

### Usage Example

```tsx
import { PdfiumViewer } from '@jayread/pdfium-viewer/core/react';
import '@jayread/pdfium-viewer/core/react/styles.css';

function PdfViewer({ pdfUrl }) {
  return (
    <PdfiumViewer
      documentUrl={pdfUrl}
      wasmUrl="/wasm/pdfium.wasm"
      plugins={[
        // Core plugins
        viewportPlugin,
        scrollPlugin,
        zoomPlugin,
        // Feature plugins
        annotationPlugin,
        searchPlugin,
        selectionPlugin
      ]}
    />
  );
}
```

## Building WASM Binaries

The PDFium WASM binary is not included in the repository. Download it using the provided scripts:

### Linux/macOS

```bash
./scripts/download-pdfium.sh
```

### Windows

```bash
./scripts/download-pdfium-windows.sh
```

These scripts:
1. Download PDFium precompiled binaries (version 144.0.7543.0) from [bblanchon/pdfium-binaries](https://github.com/bblanchon/pdfium-binaries)
2. Extract to `packages/desktop/src-tauri/pdfium/` (for desktop) or `/wasm/` (for web)
3. Include headers, libraries, and license files

See `scripts/README.md` for detailed documentation.

## Maintenance Notes

### Self-Maintained Fork

This package is **not** synced with the upstream `@embedpdf/core` via npm. All changes are vendored directly into this repository.

**Do not:**
- Update `@embedpdf/core` via npm
- Expect automatic upstream updates
- Modify the vendored PDFium bindings without testing

**Do:**
- Commit all changes directly to this workspace
- Test rendering changes across both web and desktop
- Keep PDFium WASM version in sync with desktop requirements

### Adding New Features

When adding features to the PDF viewer:

1. **Core logic** → Add to `src/core/` (framework-agnostic)
2. **React components** → Add to `src/core/react/` or `src/plugin-*/react/`
3. **Engine changes** → Update `src/engines/pdfium/`
4. **State management** → Use the store in `src/core/store/`

Always maintain cross-platform compatibility (web + desktop) since both use the same WASM engine.

### Testing

```bash
# Type checking
pnpm --filter @jayread/pdfium-viewer typecheck

# Build check
pnpm --filter @jayread/pdfium-viewer build

# Integration test (via packages/web)
pnpm --filter @jayread/web dev
```

## Dependencies

### Peer Dependencies

- **react:** >=18.0.0
- **react-dom:** >=18.0.0 (optional)

### Dev Dependencies

- **@types/react:** ^18.3.1
- **@types/react-dom:** ^18.3.0
- **react:** ^18.3.1
- **react-dom:** ^18.3.1
- **typescript:** ^5.0.0

## Related Documentation

- [JayRead Project README](../../README.md) — Overall project overview（前端集成 / Tauri 桌面集成见根目录对应章节）
- [Scripts README](../../scripts/README.md) — Build and deployment scripts

## License

This package inherits the license from the upstream embedpdf project. See [`packages/desktop/src-tauri/pdfium/licenses/`](../desktop/src-tauri/pdfium/licenses/) for PDFium licensing information.

---

**Last updated:** June 2026
**JayRead Project:** https://github.com/jayread/jayread
