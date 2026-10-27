# Zotero collection migration implementation

This implements the collection migration described in [the design analysis](zotero-collections.md). The scope is the existing local bibliography: nested collections, direct item memberships, recursive display, collection lifecycle, and its existing frontends and agent tools.

## Storage

`collection_schema.rs` migrates the legacy collection tables in a transaction after the article-to-item mappings are available. The canonical tables are now Zotero-shaped `collections`, `collectionItems`, `deletedCollections`, and `collectionRelations`.

- `collections` owns integer collectionID, collectionName, parentCollectionID, libraryID/key, version/clientVersion/synced, and clientDateModified. The public `col-*` ID and research metadata remain extensions.
- `collectionItems` references the article's native itemID and stores orderIndex plus the existing role/fetch_status/added_by/note/added_at extensions.
- `collection_articles` is a read-only compatibility view over the native tables. All application membership writes use collectionItems directly; there is no independently mirrored membership table.
- Deleted collections retain memberships for restoration. Normal reads and Unfiled ignore trashed collections. Permanent erasure requires trash first and preserves articles, attachments, annotations, and other memberships.
- Migration preserves legacy public IDs and all legacy association metadata, and does not derive hierarchy from names. Existing collections start at the library root.

Turso 0.7 lacks recursive CTE support and does not support the proposed INSTEAD OF view triggers. Tree traversal therefore uses an iterative snapshot with cycle detection. Structural validation and mutation remain within one storage actor command and transaction.

## Behavior

Direct membership is independent of hierarchy. Recursive queries deduplicate articles while retaining all active direct association provenance. Scope and full-text matching apply before total calculation and pagination. Summary queries count memberships without hydrating their IDs.

A metadata upsert preserves existing parentage. Explicit PATCH distinguishes an omitted parent_id from parent_id:null. Moves reject missing parents, self/descendant cycles, cross-library parents, trashed parents, and non-editable libraries.

User add/drag without explicit relationship changes is idempotent and preserves an existing semantic role and note. Explicit role/note editing remains available for research tools.

Collection changes publish a revision only after a successful operation. The authenticated JSON long-poll endpoint uses the same request bridge for the browser and Tauri; agent mutations on the resident shared bibliography refresh the GUI without a page reload. Separate processes also refresh on ordinary library reload.

## Interfaces

The existing `/api/v1/bib` contract now supports:

- GET /collections, with parent_id and native identity; view=summary omits member IDs.
- GET /collections/top, GET /collections/{id}/collections, and GET /collections/trash.
- POST /collections with optional parent_id.
- PATCH /collections/{id} with name and/or parent_id.
- DELETE /collections/{id} moves the subtree to trash; permanent=true erases an already trashed subtree.
- POST /collections/{id}/restore restores the subtree and necessary ancestors.
- GET /collections/{id}/articles with include_descendants, query, limit, offset, total, and association provenance.
- GET /collections/changes?since=revision waits for a committed revision or a bounded timeout.
- Import accepts collection_id and saves a new article and its membership atomically; cached articles use idempotent membership addition.
- Export uses the same query scope, supports recursive collections, and exports the complete scope when no limit is supplied.

Agent create/list/manage/export tools expose parentage, lifecycle, and recursive scope. CLI export supports --include-descendants. The TUI uses one visible tree order for rendering and navigation, Left/Right expand/collapse or traverse parents/children, and Shift+R toggles descendant membership display.

Both web frontends support nested display, persistent expansion, create/rename/move, trash/restore/erase, and server-side recursive queries. The main gateway/Tauri GUI also provides direct-membership management with path labels and article-to-collection drag/add. The built frontend resources must be synchronized before compiling the daemon that embeds them.

## Verification

Tests cover production Turso migration from legacy tables, native identities surviving repeated migration and disk reopen, semantic association preservation, FK integrity, multilevel moves, cycle rejection, subtree trash/restore/erasure, provenance retention, Unfiled, 515-article recursive search and pagination, notifications after successful operations, API behavior, TUI tree navigation, and browser interactions.

The browser test is `npm run test:collections` in apps/gui. The existing `npm run test:bib-menu` also exercises article menus, checkboxes and selective export.

A real-library validation uses SQLite's consistent backup API while the old daemon still owns the source database, migrates an isolated copy with `cargo run -p bib-base --example migrate_collections -- DATABASE`, and compares complete collection-header and association hashes before and after migration. Live rollout requires the old daemon to release its connections before the new schema is installed.

## Real-library recovery validation (2026-10-27)

The running legacy database contained 4,067 articles, 105 collection headers,
and 3,326 relationship rows. Its pre-existing FK check reported 36 violations.
The migration was validated on a consistent backup before any live switch:

- 34 dangling article references uniquely matched saved DOI/PMID identifiers.
- Two memberships referenced the same absent collection. A clearly named
  `Recovered collection col-cb3e7081` retains those relationships without
  pretending to know the missing collection's original name.
- Two aliases in one collection identify the same actual article. Their role,
  fetch status and added-by agree. The normalized membership retains the
  earliest position/date and both notes; the original rows remain auditable.
- The resulting canonical model has 106 collections and 3,325 unique
  memberships. The article count is unchanged.
- Every original collection header hashes identically. Reconstructing the
  original relationships from the canonical rows and `collectionMigrationAudit`
  produces an identical hash for every original field, including note and date.
- The migrated copy reports zero foreign-key violations.

Unresolvable or ambiguous identifiers and conflicting semantic metadata fail
and roll back the transaction. Recovery does not fetch metadata or silently
choose a different article. `collectionMigrationAudit` keeps preimages of all
repaired relationships, including the exact original identifiers.

The initial consistent backup is
`~/.autonomics/backups/bib-before-nested-collections-20261027T065025Z.db`.
Live rollout also takes a quiescent backup after the old daemon exits. Database
schema rollback requires restoring that backup together with the saved old
executable; the old binary must not run against the native schema.

The existing Zotero article backfill is now versioned, with repair detection for
missing mappings. Read-connection schema initialization no longer repeats the
entire data backfill. A creator lookup index avoids repeated full scans during
the one-time upgrade of the real library's 32,210 author associations.

## Live verification

The installed `~/autonomics` and the running daemon use the same verified
release (SHA-256 `e6d6de3948230b20d3a709e35d2790de15dfd0cea11e54f0dddd4959c69f3cde`).
The running executable embeds the current GUI asset bytes.

The live API reports 4,067 articles, 106 native collections and 3,325 unique
memberships. A read-only verification against the original consistent backup
confirms that all 105 original collection headers are unchanged, every original
relationship field can be reconstructed exactly from native rows and the
migration audit, and the live FK check has zero violations.

A real browser opened the served Bibliography screen, rendered 106 collection
rows plus Trash, selected a collection, checked its server total against the
tree count, and toggled recursive display without page errors. The installed
`autonomics bib list --limit 1` reopened the migrated live library successfully
in approximately 0.03 seconds.

A second pre-live database backup is
`~/.autonomics/backups/bib-before-live-collections-20261027T071257Z.db`;
the replaced home-directory executable is retained as
`~/.autonomics/backups/autonomics-startup-failure-20261027T071257Z`.
