# Agent transcript consistency design

Status: proposed. The existing history API and agent event stream do not yet
implement this protocol.

## Contract

Each context owns a durable, monotonically increasing transcript revision. A
committed transcript operation has a stable operation ID, context ID and
revision, and is written atomically with the transcript mutation in the same
database transaction. A transactional outbox carries the operation to the
gateway. Retrying outbox delivery preserves its ID and revision. The outbox is
retained until replay is safe; a gap or expired cursor requires a new snapshot.
The gateway's process-local SSE sequence is transport ordering, not a durable
transcript revision and not a substitute for this transaction.

The history endpoint returns the ordered messages and the revision from one
consistent database snapshot. The client subscribes before requesting that
snapshot, buffers operations, installs the snapshot, and applies operations
whose context revision is greater than the snapshot revision. Operations are
idempotent by operation ID, ordered by context revision, and a gap triggers
reconciliation. This handles commit before, during, or after the query and
delayed or repeated outbox delivery. Storage failures are errors, never empty
snapshots.

## Streaming and identity

Allocate an application message ID before a user message, model response or
tool result is emitted. Model provider IDs are metadata, not application
identity. Each stream attempt also has an ID; deltas carry message ID, attempt
ID, block identity and an increasing per-block offset. A retry or cancellation
ends its attempt explicitly. Tool calls and results reference their stable tool
call and message IDs. Multiple equal strings are distinct operations.

Streaming updates are provisional. A committed operation contains the
canonical message or block and replaces its provisional representation by ID,
including when the snapshot already contains that message. A client that sees a
commit before the delayed delta discards the delta for that completed attempt.
Partial or cancelled output must have an explicit terminal event and policy;
it cannot be treated as a committed message unless it was actually persisted.
Lifecycle, plan and transient error events remain separate from transcript
operations and must not be deduplicated by transcript revision.

## Migration and validation

Introduce the protocol as a versioned endpoint and event variant alongside
the existing array history endpoint and legacy events. Preserve old consumers
until the GUI and TUI can migrate independently. Backfill existing transcript
rows with stable IDs and revisions transactionally; retain transcript order
across compaction. Surface failed persistence and outbox delivery rather than
silently claiming a committed operation. Scope every cursor and identity to the
context, invalidate in-flight snapshots on a context change, and use bounded
buffers with resync on overflow.

Tests must exercise writes before, inside and after the snapshot transaction;
outbox delivery before and after the response; duplicate and out-of-order
delivery; replay gaps; a message already present in the snapshot; repeated
identical user messages; streaming retries and cancellation; concurrent tool
results; switching contexts; database errors; compaction; and daemon restart.
The GUI tests must verify exactly one canonical representation, continued
streaming, and no lost messages under these interleavings.
