# PMC full-text storage and PDF acquisition

## Root cause

The API import route fetched JATS XML, stripped its tags, and saved the plain
text inside `fulltexts.text_content` while setting `file_path` to
`europepmc:PMC…`. The Zotero compatibility mirror also copied that synthetic
identifier into `itemAttachments.vfsPath`. No original file existed, so the
reader's raw download endpoint correctly rejected the path as non-VFS.

The two DAG acquisition routes used a different helper that wrote plaintext to
an article-level `extracted/fulltext.txt` and then treated that mutable derived
object as the original. Neither route followed the attachment-owned,
content-addressed original-file design used for uploads. The old file-size
field also measured the XML response, rather than the served TXT bytes.

## Current acquisition route

Automatic acquisition now downloads a PDF, with no XML-to-TXT fallback:

1. Use the saved PMCID, or resolve PMID/DOI with Europe PMC metadata. MED search
   results' `pmcid` field is now retained instead of requiring a PMC-source hit.
2. Discover article-version prefixes in the current public PMC Cloud Service.
3. Read each version's metadata and its `pdf_url`; prefer a published PDF over
   an author manuscript. A larger version number alone does not imply that it
   is the publisher's version. Do not assume version 1 exists.
4. Convert documented `s3://pmc-oa-opendata/…` object URLs to the anonymous HTTPS
   endpoint. Validate the metadata's accession, version, object prefix, response
   size, and PDF signature. A missing PDF leaves the article and its existing
   attachments intact; an HTML error document is never stored as a PDF.
5. Write the original PDF to
   `vfs:///literature/{article}/attachments/{key}/{sha256}-PMC….pdf` before
   committing metadata. Store the actual byte count, SHA-256, filename,
   `application/pdf`, `storage:filename`, and the real source URL.
6. Commit the attachment, extraction state, preferred-file projection when
   appropriate, and search index through one storage actor transaction. A
   previously selected user upload remains preferred. Upgrading automatic OA
   text creates a new PDF attachment and preserves the old text attachment and
   its annotations.
7. Start the existing attachment-scoped Zotero SDT → MinerU extraction chain.
   The PDF remains readable even if text extraction fails; extracted text and
   semantic sidecars remain derived objects under the attachment key.

API import (including cached records), `bib_save`, and `literature_fulltext`
use the same acquisition-and-storage service. Repeated acquisition reuses a
previously stored PDF. Metadata-only bibliography instances do not fabricate
file-backed results when no VFS is configured.

## Legacy TXT repair

Saved text is materialized into a real attachment-owned, content-addressed TXT
file plus a separate extracted-text object. The attachment key, existing
annotations, inline text and source category survive; the byte count and hash
are recomputed from the actual UTF-8 text. A real Europe PMC source URL replaces
the pseudo URL. This preserves the historical plain-text representation; it
does not pretend that saved stripped text is an original PDF or original JATS.

The startup sweep repairs all eligible legacy attachments before extraction
recovery. Both preferred-file and attachment-key raw download endpoints also
repair on read, so an early reader request does not depend on sweep completion.
Repair uses an expected-path check in the database transaction: a concurrent
upload or preferred-file change is not overwritten.

An explicit maintenance command uses the runtime's mounted VFS configuration:

```sh
autonomics bib repair-storage
```

## Verification and upstream contract

Tests cover PDF version selection, S3 metadata URLs, no-PDF responses, rejection
of HTML and unrelated URLs, original-byte storage, retained TXT/annotations,
user-preferred uploads, failed storage writes, idempotent TXT repair, and raw
TXT GET/HEAD/Range/attachment routes with UTF-8 byte lengths.

The isolated real-network check is:

```sh
cargo run -p bib-base --example pmc_pdf -- PMC13396644
```

It downloaded a 2,290,208-byte PDF, stored/read the exact bytes, and verified
SHA-256 `0f71eac5d0c79bb44f9ff808dceae229362b962da5bac31149b2b88fdca89552`.

Live verification on 2026-10-27 also confirmed:

- All 112 eligible legacy attachments were repaired. Their attachment keys,
  UTF-8 text hashes and byte lengths survived; every attachment's authenticated
  raw download returned the exact original bytes. No `europepmc:` VFS paths
  remain. Article and annotation counts were unchanged by repair, and the
  database foreign-key check passed.
- Cached PMID `40241176` automatically acquired `PMC12004813.1.pdf`
  (2,733,760 bytes), keeping its historical TXT attachment. Its raw download
  served `application/pdf`; the real gateway GUI rendered all 16 page canvases
  without page errors.
- Before maintenance, the database was backed up to
  `/home/wjx/.autonomics/backups/bib-before-pmc-pdf-20261027T074708Z.db`.
  The previous executable was retained beside it, and the new daemon's running
  executable hash was verified against the installed release.

The implementation follows the current [PMC cloud documentation](https://pmc.ncbi.nlm.nih.gov/tools/pmcaws/)
and its [technical README](https://pmc-oa-opendata.s3.amazonaws.com/README.txt),
including metadata-provided PDF URLs and the distinction between manuscripts
and published versions. PMC retired its legacy OA Web Service/FTP distribution
in August 2026; the retired `oa.fcgi` endpoint and deprecated packages are not
used.
