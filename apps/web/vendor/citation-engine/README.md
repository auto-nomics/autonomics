# @jayread/citation-engine

> **CSL citation engine for the JayRead project** — Client-side citeproc-js wrapper with React hooks

A frontend package that provides CSL (Citation Style Language) rendering capabilities using citeproc-js. This package handles academic citation formatting and bibliography generation for research papers in the JayRead project.

## Package Information

- **Name:** `@jayread/citation-engine`
- **Version:** 0.5.4
- **Private:** true (JayRead workspace internal package)
- **Type:** ESM module
- **Entry:** `./src/index.ts`

## Purpose

This package provides client-side citation rendering for JayRead's research paper management system. It wraps [citeproc-js](https://github.com/Juris-M/citeproc-js) to offer:

- **On-demand citation rendering** — Lazy-loads the CSL engine only when citations are needed
- **Multiple citation styles** — Bundles 5 CSL styles (GB/T 7714, IEEE, APA, AMA, Nature)
- **React integration** — Provides `useCitationEngine` and `useCitation` hooks for React components
- **Offline support** — All CSL styles and locales bundled (~500KB) for offline use
- **LRU caching** — Process-wide engine pool (max 3) to avoid redundant initialization overhead

## Backend Integration

This package works with the backend [`jayread-citation`](../server-rs/crates/jayread-citation/) crate:

- **Backend responsibility:** Generate CSL-JSON data from paper metadata (via `jayread-citation`)
- **Frontend responsibility:** Consume CSL-JSON and render formatted citations (this package)

The separation allows the frontend to render citations offline while the backend handles citation data extraction and CSL-JSON generation.

## Public API

The package exports the following modules (via `package.json` exports):

```typescript
// Main entry point
import { CitationEngine } from '@jayread/citation-engine';
import type { CitationResult, BibliographyResult, CiteprocItem } from '@jayread/citation-engine';

// React hooks
import { useCitationEngine, useCitation } from '@jayread/citation-engine/react';

// Bundled CSL styles
import { getBundledStyle, BUNDLED_STYLES } from '@jayread/citation-engine/styles';

// Locale data
import { getLocaleXml, SUPPORTED_LOCALES } from '@jayread/citation-engine/locales';
```

### Core Classes

#### `CitationEngine`

Main engine class that wraps citeproc-js for a single CSL style:

```typescript
class CitationEngine {
  constructor(styleXml: string, localeXml: string);

  // Add items to the engine
  addItems(items: CiteprocItem[]): void;

  // Render a citation cluster
  citeCluster(itemIds: string[], locators?: CitationClusterLocator[]): CitationResult;

  // Generate bibliography
  makeBibliography(): BibliographyResult;
}
```

#### React Hooks

##### `useCitationEngine(styleId?)`

Manages engine lifecycle with lazy-loading and LRU caching:

```typescript
const { engine, loading, error } = useCitationEngine('apa');
```

- Returns a cached `CitationEngine` instance for the specified style
- Engine is created asynchronously on mount (deferred to next tick)
- Process-wide LRU pool (max 3 engines) reuses instances across components

##### `useCitation(itemId, fetchItem, options?)`

Convenience hook that fetches CSL-JSON and renders a citation:

```typescript
const { text, loading, error } = useCitation(
  paperId,
  async (id) => {
    const response = await fetch(`/api/papers/${id}/csl`);
    return response.json();
  },
  { styleId: 'gb-t-7714', locator: { label: 'page', locator: '123' } }
);
```

## Bundled Resources

The package includes CSL style definitions and locale data for offline use:

### Citation Styles (5)

| Style ID | Name | Locale | Default |
|----------|------|--------|---------|
| `gb-t-7714-2015-numeric` | China National Standard GB/T 7714-2015 (numeric) | zh-CN | ✅ |
| `ieee` | IEEE | en-US | |
| `apa` | American Psychological Association 7th edition | en-US | |
| `american-medical-association` | American Medical Association 11th edition | en-US | |
| `nature` | Nature | en-US | |

### Supported Locales (2)

- `en-US` — English (United States)
- `zh-CN` — Chinese (Simplified)

## Usage Examples

### Basic Citation Rendering

```tsx
import { useCitation } from '@jayread/citation-engine/react';

function PaperCitation({ paperId }) {
  const { text, loading, error } = useCitation(paperId, async (id) => {
    const response = await fetch(`/api/papers/${id}/csl`);
    return response.json();
  }, { styleId: 'apa' });

  if (loading) return <div>Loading citation...</div>;
  if (error) return <div>Error: {error.message}</div>;
  return <div dangerouslySetInnerHTML={{ __html: text || '' }} />;
}
```

### Direct Engine Usage

```typescript
import { CitationEngine } from '@jayread/citation-engine';
import { getBundledStyle } from '@jayread/citation-engine/styles';
import { getLocaleXml } from '@jayread/citation-engine/locales';

const style = getBundledStyle('apa');
const localeXml = getLocaleXml(style.locale);
const engine = new CitationEngine(style.cslXml, localeXml);

// Add CSL-JSON items
engine.addItems([
  { id: 'item-1', type: 'article-journal', title: 'Paper Title', ... }
]);

// Render citation
const { html } = engine.citeCluster(['item-1']);
console.log(html); // "<i>Paper Title</i>..."
```

### Custom Style Management

```tsx
import { useCitationEngine } from '@jayread/citation-engine/react';
import { BUNDLED_STYLES } from '@jayread/citation-engine/styles';

function CitationStyleSelector() {
  const [styleId, setStyleId] = useState('gb-t-7714');
  const { engine, loading } = useCitationEngine(styleId);

  return (
    <select value={styleId} onChange={(e) => setStyleId(e.target.value)}>
      {BUNDLED_STYLES.map(style => (
        <option key={style.id} value={style.id}>
          {style.name}
        </option>
      ))}
    </select>
  );
}
```

## Architecture

### Directory Structure

```
src/
├── engine.ts      # CitationEngine class (citeproc-js wrapper)
├── react.ts       # React hooks (useCitationEngine, useCitation)
├── styles.ts      # Bundled CSL style definitions
├── locales.ts     # Locale data (en-US, zh-CN)
└── index.ts       # Main entry point
types/
└── citeproc.d.ts  # TypeScript declarations for citeproc
```

### Design Decisions

1. **Bundled resources** — CSL styles (~300KB) and locales (~200KB) are bundled directly in the package for offline use
2. **Lazy loading** — Engine initialization is deferred until first citation request to keep initial page load fast
3. **LRU caching** — Process-wide engine pool (max 3) avoids redundant initialization when multiple components use the same style
4. **React-agnostic core** — `CitationEngine` class is framework-agnostic; React integration is provided via separate hooks

## Dependencies

### Runtime Dependencies

- **citeproc:** ^2.4.63 — CSL citation processor

### Peer Dependencies

- **react:** >=18.0.0 (optional, required only for React hooks)

### Development Dependencies

- **@types/react:** ^18.3.1
- **react:** ^18.3.1
- **typescript:** ^5.0.0
- **vitest:** ^1.0.0

## Building and Testing

```bash
# Type checking
pnpm --filter @jayread/citation-engine typecheck

# Run tests
pnpm --filter @jayread/citation-engine test

# Build check
pnpm --filter @jayread/citation-engine build
```

## Integration in JayRead

This package is consumed by `packages/web` as a workspace dependency:

```json
{
  "dependencies": {
    "@jayread/citation-engine": "workspace:*"
  }
}
```

### Backend Integration Points

- **CSL-JSON generation:** `packages/server-rs/crates/jayread-citation` extracts citation data from PDF metadata
- **API endpoint:** `GET /api/papers/{id}/csl` returns CSL-JSON for a paper
- **Frontend consumption:** React components use `useCitation` hook to fetch and render

## Maintenance Notes

### Adding New Citation Styles

1. Obtain CSL XML from [CSL Repository](https://github.com/citation-style-language/styles)
2. Drop the `.csl` file into `assets/styles/` (repo root of this package, not under `src/`)
3. Export in `src/styles.ts` via `BUNDLED_STYLES`
4. Rebuild package

### Adding New Locales

1. Obtain locale XML from [CSL Locale Repository](https://github.com/citation-style-language/locales)
2. Add to `src/assets/locales/`
3. Export in `src/locales.ts` via `SUPPORTED_LOCALES`

### Performance Considerations

- **Engine pool size** — Currently capped at 3; adjust `POOL_MAX_SIZE` in `react.ts` if needed
- **Bundle size** — Total bundle is ~500KB (citeproc + styles + locales); code splitting is already applied
- **Lazy loading** — Engine initialization is ~50-200ms; hooks defer this to avoid blocking initial render

## Related Documentation

- [JayRead Project README](../../README.md) — Overall project overview
- [Web Package README](../web/README.md) — Frontend integration
- [jayread-citation crate](../server-rs/crates/jayread-citation/) — Backend CSL-JSON generation
- [citeproc-js documentation](https://github.com/Juris-M/citeproc-js) — Underlying citation processor

## License

This package is part of the JayRead project. See the root LICENSE file for details.

---

**Last updated:** June 2026
**JayRead Project:** https://github.com/jayread/jayread
