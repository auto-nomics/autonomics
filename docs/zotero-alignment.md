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
- `itemAnnotations` represents PDF highlights/comments as child items of an
  attachment. `itemNotes` represents article-level child notes when no
  attachment is available.

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

Deletes remove the mapped item tree as well as the legacy rows. The next phase
is to move per-attachment extraction and search state fully behind the
attachment key, then migrate saved searches and trash to the Zotero object
model.
