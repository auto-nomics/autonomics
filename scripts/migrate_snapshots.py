#!/usr/bin/env python3
"""One-shot migration: convert old Memory snapshots to SessionState format.

Old format (memory column JSON):
  {"items": [{"messages": [...], "summary": null}]}

New format (same column, different JSON):
  {"messages": [...], "summary": null, "ancestor_summaries": []}

Also handles multi-segment snapshots (shouldn't exist but defensive):
  {"items": [
    {"messages": [], "summary": "old summary"},
    {"messages": [...], "summary": null}
  ]}
→
  {"messages": [...], "summary": null, "ancestor_summaries": ["old summary"]}
"""

import json
import sqlite3
import sys
import os
from pathlib import Path


def migrate(db_path: str, dry_run: bool = False) -> None:
    if not Path(db_path).exists():
        print(f"Database not found: {db_path}")
        sys.exit(1)

    print(f"Opening {db_path}...")
    conn = sqlite3.connect(db_path)

    # Check column name (could be "memory" in old schema)
    cursor = conn.execute("PRAGMA table_info(snapshots)")
    cols = {row[1]: row for row in cursor.fetchall()}
    if "memory" not in cols:
        print("No 'memory' column in snapshots table — already migrated?")
        conn.close()
        return

    col_name = "memory"

    # Read all old-format snapshots
    cursor = conn.execute(
        f"SELECT snapshot_id, {col_name} FROM snapshots "
        f"WHERE {col_name} LIKE '%\"items\"%'"
    )
    rows = cursor.fetchall()
    total = len(rows)
    print(f"Found {total} old-format snapshots to migrate.")

    if total == 0:
        print("Nothing to migrate.")
        conn.close()
        return

    if dry_run:
        print("[DRY RUN] No writes will be performed.")

    migrated = 0
    errors = 0
    batch = []
    BATCH_SIZE = 500

    for snapshot_id, memory_json in rows:
        try:
            old = json.loads(memory_json)
            items = old.get("items", [])

            if not items:
                # Empty memory — create empty SessionState
                new_state = {
                    "messages": [],
                    "summary": None,
                    "ancestor_summaries": [],
                }
            else:
                # Collect ancestor summaries (all segments except the last)
                ancestor_summaries = []
                for item in items[:-1]:
                    s = item.get("summary")
                    if s:
                        ancestor_summaries.append(s)

                # Last segment is the current one
                last = items[-1]
                new_state = {
                    "messages": last.get("messages", []),
                    "summary": last.get("summary"),
                    "ancestor_summaries": ancestor_summaries,
                }

            new_json = json.dumps(new_state, separators=(",", ":"))
            batch.append((new_json, str(snapshot_id)))
            migrated += 1

            if len(batch) >= BATCH_SIZE:
                if not dry_run:
                    conn.executemany(
                        f"UPDATE snapshots SET {col_name} = ? WHERE snapshot_id = ?",
                        batch,
                    )
                    conn.commit()
                batch.clear()
                print(f"  Progress: {migrated}/{total}")

        except (json.JSONDecodeError, KeyError, TypeError) as e:
            errors += 1
            if errors <= 5:
                print(f"  ERROR on snapshot {snapshot_id}: {e}")

    # Flush remaining batch
    if batch and not dry_run:
        conn.executemany(
            f"UPDATE snapshots SET {col_name} = ? WHERE snapshot_id = ?",
            batch,
        )
        conn.commit()

    # Optionally rename the column from "memory" to "state"
    # (SQLite doesn't support RENAME COLUMN before 3.25, but modern versions do)
    if not dry_run:
        try:
            conn.execute(f"ALTER TABLE snapshots RENAME COLUMN {col_name} TO state")
            print(f"Renamed column '{col_name}' → 'state'")
        except sqlite3.OperationalError as e:
            # Column rename may fail if it's already named 'state' or
            # if there are constraints. Non-fatal.
            print(f"Column rename skipped: {e}")

    conn.close()
    print(f"\nMigration complete: {migrated} migrated, {errors} errors, {total} total.")
    if dry_run:
        print("[DRY RUN] No changes were written.")


if __name__ == "__main__":
    default_db = os.path.expanduser("~/.autonomics/agent.db")
    db_path = sys.argv[1] if len(sys.argv) > 1 else default_db
    dry_run = "--dry-run" in sys.argv
    migrate(db_path, dry_run)
