# React Integration for PDF Viewer

This directory contains the React version of the PDF Viewer components, exported via `@jayread/pdfium-viewer/core/react`.

## Installation

`@jayread/pdfium-viewer` is a private workspace package — it is consumed directly by `packages/web` and `packages/desktop` through the pnpm workspace, not installed from npm.

## Usage

Import the React components from the `core/react` entry point:

```tsx
import { EmbedPDF, Viewport } from '@jayread/pdfium-viewer/core/react';

function MyPDFViewer() {
  return (
    <EmbedPDF
      engine={/* your PDF engine */}
      onInitialized={async (registry) => {
        // Initialize your viewer
      }}
      plugins={(viewportElement) => [
        // Your plugins (e.g. from @jayread/pdfium-viewer/plugin-viewport/react)
      ]}
    >
      <Viewport className="my-viewport" />
    </EmbedPDF>
  );
}
```

## Hooks

You can also use the provided hooks:

```tsx
import { useCapability } from '@jayread/pdfium-viewer/core/react';

function MyComponent() {
  const zoom = useCapability('zoom');

  return <button onClick={() => zoom.zoomIn()}>Zoom In</button>;
}
```
