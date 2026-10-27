# Zotero alignment

The bibliography storage is being migrated to Zotero's object model without
abandoning Autonomics' VFS-backed object store.

## Current model

Opening `bib.db` creates and populates these Zotero-compatible tables:

- `libraries` owns permissions, versions, and storage versions.
- `items` is the root object table with a stable `(libraryID, key)`.
- `fields`, `itemDataValues`, and `itemData` hold normalized item metadata.
- `itemCreators`, `creatorTypes`, `tags`, and `itemTags` hold contributors and
  keywords.
- `itemAttachments` represents an attachment as a child item with Zotero link
  modes and path metadata.
- `fulltextItems` records index coverage, index version, sync state, and
  extraction provenance.
- `fulltextItems` also stores the attachment-keyed extracted text and state used
  by extraction and search.
- `itemAnnotations` represents PDF highlights/comments as child items of an
  attachment. `itemNotes` represents article-level child notes when no
  attachment is available.
- `searchTerms` is keyed by Zotero item keys: attachment keys own full-text and
  PDF annotation terms, while the article item key owns title/abstract terms.
  `articleID` remains only as a result-aggregation projection for the existing
  article list API.

The existing `articles` and `fulltexts` tables are a compatibility layer for
the current API and DAG nodes. Article and full-text writes now mirror into the
new tables in the same database transaction. The mapping is in `articleItems`.
`articleItems.attachmentItemID` identifies the preferred/full-text attachment;
the one-to-many source of truth is `itemAttachments.parentItemID`.

The bibliography API now exposes the child-item model directly:

- `GET /api/v1/bib/articles/{id}/attachments`
- `POST /api/v1/bib/articles/{id}/attachments`
- `GET /api/v1/bib/articles/{id}/attachments/{key}`
- `GET /api/v1/bib/articles/{id}/attachments/{key}/raw`
- `PUT /api/v1/bib/articles/{id}/attachments/{key}/preferred`
- `DELETE /api/v1/bib/articles/{id}/attachments/{key}`

An upload with `preferred=true` (the default) creates a new attachment, makes
it the compatibility full text, and runs the existing extraction chain. With
`preferred=false`, it remains a separate attachment child item.

## VFS extension

Zotero's `path` remains Zotero-shaped (`storage:paper.pdf`). Autonomics adds:

- `itemAttachments.vfsPath`: the canonical resolvable `vfs:///literature/...`
  URI.
- `itemAttachments.originalFilename`: the decoded user-facing filename.
- `fulltextItems.vfsExtractedPath`: the derived markdown/plain-text artifact.
- Derived objects can instead use the attachment-scoped canonical path
  `.../attachments/{key}/extracted/fulltext.{md|txt}`.

This preserves content-addressed object storage while keeping attachment
identity and link semantics separate from physical object location.

## Migration

Migration runs idempotently on every open:

1. Each legacy article gets one `journalArticle` item.
2. Its metadata is mirrored into `itemData`; authors and keywords become item
   creators and tags.
3. Each legacy full text becomes the article's preferred imported-file
   attachment (or imported URL for a legacy non-VFS pointer).
4. Its extraction state is mirrored into `fulltextItems`.
5. Legacy inline extracted text is copied into the preferred attachment's
   `fulltextItems.textContent` row and materialized under its attachment key.

Extraction claiming, success/failure recording, startup sweeping, derived text,
and figures all operate by attachment key. Legacy article-ID methods delegate to
the preferred attachment and mirror its state into `fulltexts` for compatibility.
The next phase is to migrate saved searches and trash to the Zotero object
model.

## Structured text and Reading Mode

PDF extraction now prefers the directly vendored Zotero document-worker and its
ONNX paragraph/block models. The complete SDT is stored under the attachment
key, with a paragraph-preserving fulltext projection. The GUI uses Zotero's
semantic renderer and PDF/SDT mapper for reflow and annotations. Existing PDFs
can be upgraded on demand when Reading Mode is first opened. See
[Zotero Reading Mode](zotero-reading-mode.md) for source analysis, cache/API
contracts, migration behavior, verification and limitations.

## Nested collections

Collections now use Zotero-shaped native identities, parent references,
`collectionItems`, and `deletedCollections`. Existing research-specific
association metadata is retained as extensions, and legacy public IDs remain
stable. See [collection implementation](zotero-collections-implementation.md)
for migration, lifecycle, API, frontend, and verification details.
