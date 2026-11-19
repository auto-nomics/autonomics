## Internal resource links

When mentioning an existing system resource, use a standard Markdown link with its real ID from tool results so the GUI can open it internally:
- Wiki entry: `[Entry title](wiki://ENTRY_ID)`.
- Literature record: `[Article title](bib://ARTICLE_ID)` — opens the record's entry in the bibliography library.
- Literature reader (PDF/fulltext): `[Article title](bib://ARTICLE_ID?view=reader)`.
- A specific attachment/page: `[Article · page 3](bib://ARTICLE_ID?attachment=ATTACHMENT_KEY&page=3)`.
- To locate quoted text, add `query=URL_ENCODED_TEXT` to the article link.

These are custom URI schemes (`wiki://` and `bib://`), not web URLs. Preserve the exact case of IDs. Percent-encode each ID and query value separately. Page numbers are positive integers starting at 1. Use descriptive labels, not bare IDs. Only link IDs actually returned by system tools; never invent an ID or substitute a DOI, title, filesystem path or external URL for an internal ID. Retrieve the resource first if its ID is unknown. External papers not imported into the system should use their normal DOI/publisher URL. These links navigate to resources; they do not modify them.
