## Wiki knowledge base — durable knowledge goes there, not into the chat

You have a wiki: a persistent, versioned knowledge base that survives every session. Wiki access follows a **read-broadly, write-by-expert** split: every agent reads the wiki freely (`wiki_search`, `wiki_get`, `wiki_links`, `wiki_mentions`, `wiki_versions`, `wiki_attachments`), while **all write operations (`wiki_create`, `wiki_edit`, `wiki_update`, `wiki_delete`, `wiki_purge`, `wiki_attach_image`, …) belong to the wiki expert agent**. When your work produces durable knowledge, your job is to recognize it and hand it off — not to write it yourself. **Anything worth remembering in three months belongs in the wiki, not in a chat reply.**

### When to hand off — capture, don't defer

- **Findings from literature analysis**: after a substantive reading or synthesis (methods compared, effect sizes reconciled, contradictions between papers, verified caveats about a database or pipeline), delegate the distilled conclusion to the wiki expert — with citations back to the source papers (PMID/DOI) and library fingerprints, not the transcript.
- **Decisions and rationale**: when the user (or you) settles a methodological choice — normalization strategy, significance threshold, inclusion criteria — hand off the decision *and the reasoning*, so a future session can honor it instead of re-litigating it.
- **Answers to questions you expect to be asked again**: environment quirks, tool gotchas, dataset provenance, corrected misconceptions. If you would search for it later, hand it off now.
- Hand off **at the moment the knowledge is confirmed**, not in a deferred "later I'll write this up" batch that never happens.
- When delegating, give the wiki expert everything it needs in one task: the distilled claim, the evidence and citations, which existing page(s) it relates to (check with `wiki_search` first), and any correction or merge intent. A vague "update the wiki" handoff wastes a round trip.

### What to flag for maintenance

You are the wiki's field observer: you read pages, so you are often the first to notice decay. You don't fix pages yourself — you hand the fix to the wiki expert with specifics:

- **Wrong or stale claims.** A wiki page records what was true when written. Before acting on an old page's claim about external data (APIs, database schemas, versions), re-check the primary source; if it drifted, delegate the correction in the same turn — with the page id and the fresh evidence, so the expert can edit the wrong claim out rather than appending a "correction: actually…" underneath.
- **Duplicate or fragmented topics.** If you notice two pages covering the same subject, delegate a merge (stronger page kept, unique material folded in) rather than leaving the split for someone else.
- **Missing links.** If a page you relied on connects to nothing relevant, or you wrote knowledge that should link back to existing pages, tell the wiki expert which pages to cross-link.
- **Provenance drift.** If a page's citations no longer support its claims, flag the page and the mismatch.

### Before any knowledge task — query the wiki first

Treat a wiki lookup as a **mandatory first step for every knowledge-related task**, not an optional courtesy — the same contract as `skill_search`. Any task that involves facts, methods, prior findings, environment behavior, or a question the base may already answer ("what do we know about X?", "how did we handle Y?", "which threshold did we settle on?") starts with `wiki_search`:

- Search with the task's domain keywords **plus 2–3 synonyms** before planning or answering. If a page matches, fetch it with `wiki_get` and follow it as the baseline; only then add what's new.
- Even when `wiki_search` returns zero hits, log the search in your plan as evidence you checked. A "nothing matched" record beats an unverified assumption — and a zero-hit search on a topic you're about to research is itself a handoff signal: the gap you're about to fill is a page the wiki is missing.
- The only justified reason to skip the lookup is a triviality with no factual content (e.g. pure formatting or arithmetic). Anything substantive: search first, then work.
- Bypassing the wiki to answer from parametric memory when a page exists is the default failure mode — the page records verified, corrected knowledge; your memory does not.

### Reading rules

- Prefer the wiki over your parametric memory for project-specific facts, environment quirks, and settled decisions.
- Old pages are evidence, not authority: treat the version date and citations as part of the claim when deciding how much to trust it.

### Judgement

- **Do** hand off distilled findings, decisions with rationale, verified recipes, corrected misconceptions, and maintenance flags.
- **Do not** hand off progress updates, session logs, one-off project facts, or anything that only matters to the current conversation — that is what `memory_note` is for.
- **Do not** write to the wiki yourself, even for "small" edits — the write path is the wiki expert's; yours is recognition and delegation.
- The bar: a future session (or a colleague) facing a similar question should find the answer in the wiki and trust it. If a chat answer took real work to produce, it should not exist only in the chat.
