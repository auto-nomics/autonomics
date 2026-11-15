# Bibliography data reliability

The GUI and `bib_delete` agent tool move records to recoverable trash. Metadata, collection memberships, annotations and attachment objects remain intact. Active listings, collection counts, exports and lexical search omit trashed records. The search projection excludes the entire record family, including attachment and annotation documents.

## HTTP contract

All paths below are relative to `/api/v1/bib`.

- `GET /articles/{id}` returns the existing detail payload plus `revision`, a SHA-256 digest of canonical serialized metadata. Missing and trashed records return 404.
- `PUT /articles/{id}` accepts the flat Article payload plus optional `expected_revision`. A supplied token is checked in the same immediate database transaction as the update. A stale token or identifier collision returns 409; validation errors return 400. The response contains `article` and its canonical `revision`.
- `POST /articles/manual` accepts an Article (`id` may be empty), generates a UUID and timestamps, and atomically checks identifier collisions before insertion. It returns 201 with `article` and `revision`. No external identifier is required.
- `DELETE /articles/{id}` moves the record to trash. Repeated deletion of a still-existing trashed record is harmless.
- `GET /articles/trash?limit=50&offset=0` returns `{articles:[{article,deleted_at}],total}`. Maximum page size is 500.
- `POST /articles/{id}/restore` restores the record and its preserved memberships. A collection that is itself in trash still needs separate restoration.
- `DELETE /articles/{id}/permanent` requires the record to be in trash. It atomically removes the database family, then cleans original files and derived attachment assets. Object-storage cleanup failures are logged; they cannot roll back the database deletion.

Attachment/fulltext routes and article-scoped annotation routes return 404 while the parent is in trash. Internal storage methods remain accessible to restoration and cleanup code. Existing low-level `BibBase::delete_article` is a permanent-delete primitive; user-facing tools use `set_article_trash` instead.

For backwards compatibility, metadata PUT requests without `expected_revision` remain accepted. New editors and offline clients must supply the detail token. The token reflects all metadata writers, including ordinary imports and agent edits, without requiring each writer to update a separate revision counter. It does not include attachment bytes or annotation changes; it protects metadata edits only.

## Additional metadata

`Article.extra_fields` is a default-empty ordered string map for type-specific fields, including publisher, publication place, edition, book title, university, institution, report type, patent number, website title, access date and URL. Values persist in `articleExtraFields`, a separate table keyed to the article with cascading deletion. Empty maps are omitted from JSON to retain previous evidence serialization. The GUI loads complete details before editing so hidden fields survive save.

## Verification

`crates/bib-base/tests/reliability.rs` exercises concurrent/stale edits, atomic duplicate-ID creation, trash visibility with both search engines, restoration, permanent deletion, disk reopen/index rebuild, and type-specific metadata round trips. `crates/api-server/tests/bib_reliability.rs` covers the HTTP contract and preservation/removal of two actual attachment files. Existing API and agent-deletion tests cover the compatibility surface.
