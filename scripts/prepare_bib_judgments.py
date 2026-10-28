#!/usr/bin/env python3
"""Blind candidate pooling and strict, declared-human BEIR judgment export.

No query text, relevance grades, or positive labels are generated. Inputs are
local corpus/query JSONL and qrels-free retrieval runs. Only review/ is shared
with assessors; admin/ records split assignment, source hashes and pool origin.
"""
import argparse
import csv
import hashlib
import json
from pathlib import Path
import shutil

from evaluate_bib_relevance import load_run, records, sha256

FIELDS = ["pool_id", "query_id", "query", "doc_id", "title", "text", "source", "preview_only", "grade", "note", "assessor_id"]
ADJUDICATION = ["query_id", "doc_id", "grade", "note", "assessor_id"]


def csv_write(path, fields, rows):
    with path.open("w", encoding="utf-8", newline="") as output:
        writer = csv.DictWriter(output, fields)
        writer.writeheader()
        writer.writerows(rows)


def csv_read(path, fields):
    with Path(path).open(encoding="utf-8-sig", newline="") as source:
        reader = csv.DictReader(source)
        if reader.fieldnames != fields:
            raise ValueError(f"unexpected review columns: {path}")
        rows = list(reader)
        if any(set(row) != set(fields) or any(not isinstance(value, str) for value in row.values()) for row in rows):
            raise ValueError("malformed review row")
        return rows


def pool(corpus_path, queries_path, runs, directory, top_k=30, seed="bib-judgments-v1"):
    if not 1 <= top_k <= 100:
        raise ValueError("pool depth must be 1..100")
    corpus = {row["_id"]: row for row in records(corpus_path)}
    queries = {row["_id"]: row for row in records(queries_path)}
    if not corpus or not queries or not runs:
        raise ValueError("nonempty corpus, queries and retrieval runs required")
    groups = {}
    splits = {}
    for query, row in queries.items():
        if not isinstance(row.get("text"), str) or not row["text"].strip():
            raise ValueError("queries must contain actual task text")
        group = row.get("group_id", "text:" + hashlib.sha256(" ".join(row["text"].split()).encode()).hexdigest())
        if not isinstance(group, str) or not group:
            raise ValueError("invalid query group")
        assigned = "test" if int(hashlib.sha256(f"{seed}\0{group}".encode()).hexdigest(), 16) % 2 else "train"
        split = row.get("split", assigned)
        if split not in ("train", "test") or groups.setdefault(group, split) != split:
            raise ValueError("query group crosses tuning and acceptance splits")
        splits[query] = split
    if set(splits.values()) != {"train", "test"}:
        raise ValueError("both tuning and acceptance tasks are required")
    origins = {}
    run_hashes = {}
    for name, path in runs:
        if not name or name in run_hashes:
            raise ValueError("pool run names must be unique")
        run = load_run(path, queries, corpus)
        run_hashes[name] = sha256(path)
        for query, documents in run.items():
            for rank, doc in enumerate(documents[:top_k], 1):
                origins.setdefault((query, doc), []).append({"run": name, "rank": rank})
    if set(query for query, _ in origins) != set(queries):
        raise ValueError("a query has no candidates; expand its pool before review")
    binding = {"seed": seed, "query_splits": splits, "run_sha256": run_hashes,
               "corpus_sha256": sha256(corpus_path), "queries_sha256": sha256(queries_path), "top_k": top_k}
    pool_id = hashlib.sha256(json.dumps(binding, sort_keys=True).encode()).hexdigest()
    rows = []
    for query, doc in sorted(origins, key=lambda pair: hashlib.sha256(f"{seed}\0{pair[0]}\0{pair[1]}".encode()).digest()):
        source = corpus[doc]
        text = source.get("text", "")
        if not isinstance(text, str) or not isinstance(source.get("title", ""), str):
            raise ValueError("paper title and text must be strings")
        rows.append(dict(zip(FIELDS, [pool_id, query, queries[query]["text"], doc, source.get("title", ""), text[:16000],
                                     source.get("url", ""), "yes" if len(text) > 16000 else "no", "", "", ""])))
    root = Path(directory)
    root.mkdir(exist_ok=False)
    (root / "review").mkdir()
    (root / "admin").mkdir()
    csv_write(root / "review/tasks.csv", FIELDS, rows)
    for name in ("corpus", "queries"):
        shutil.copyfile(corpus_path if name == "corpus" else queries_path, root / "admin" / f"{name}.jsonl")
    provenance = [{"query_id": q, "doc_id": d, "origins": origin} for (q, d), origin in sorted(origins.items())]
    (root / "admin/pool-provenance.json").write_text(json.dumps(provenance, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
    manifest = {"schema": "bib-blind-judgment-pool-v1", "ready_for_relevance_evaluation": False,
                "relevance_grades_generated": False, "seed": seed, "query_splits": splits,
                "query_count": len(queries), "pair_count": len(rows), "top_k_per_run": top_k,
                "run_sha256": run_hashes,
                "files_sha256": {p: sha256(root / p) for p in ["review/tasks.csv", "admin/corpus.jsonl", "admin/queries.jsonl", "admin/pool-provenance.json"]}}
    (root / "manifest.json").write_text(json.dumps(manifest, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
    return manifest


def assessment(path, original):
    result = {}
    assessors = set()
    for row in csv_read(path, FIELDS):
        pair = (row["query_id"], row["doc_id"])
        if pair not in original or pair in result or any(row[key] != original[pair][key] for key in FIELDS[:-3]):
            raise ValueError("review rows changed identity/content, or contain unknown/duplicate pairs")
        if row["grade"] not in ("0", "1", "2", "3") or not row["assessor_id"].strip():
            raise ValueError("every pair requires an explicit grade and assessor; blank is not zero")
        if int(row["grade"]) > 0 and not row["note"].strip():
            raise ValueError("positive grades require a supporting note/source location")
        result[pair] = int(row["grade"])
        assessors.add(row["assessor_id"].strip())
    if set(result) != set(original) or len(assessors) != 1:
        raise ValueError("each assessor must independently review the complete pool")
    return result, next(iter(assessors))


def export(pool_directory, assessments, directory, declaration, adjudication=None):
    root = Path(pool_directory)
    manifest = json.loads((root / "manifest.json").read_text(encoding="utf-8"))
    if manifest.get("schema") != "bib-blind-judgment-pool-v1" or not declaration.strip():
        raise ValueError("a frozen pool and explicit judgment provenance declaration are required")
    for name, digest in manifest["files_sha256"].items():
        if sha256(root / name) != digest:
            raise ValueError("frozen pool content changed")
    if len(assessments) != 2:
        raise ValueError("two independent assessor files are required")
    original = {(r["query_id"], r["doc_id"]): r for r in csv_read(root / "review/tasks.csv", FIELDS)}
    binding = {"seed": manifest["seed"], "query_splits": manifest["query_splits"], "run_sha256": manifest["run_sha256"],
               "corpus_sha256": manifest["files_sha256"]["admin/corpus.jsonl"],
               "queries_sha256": manifest["files_sha256"]["admin/queries.jsonl"], "top_k": manifest["top_k_per_run"]}
    pool_id = hashlib.sha256(json.dumps(binding, sort_keys=True).encode()).hexdigest()
    if any(row["pool_id"] != pool_id for row in original.values()):
        raise ValueError("pool configuration or split assignment changed after freezing")
    first, a = assessment(assessments[0], original)
    second, b = assessment(assessments[1], original)
    if a == b:
        raise ValueError("assessor identities must be distinct")
    conflicts = {pair for pair in first if first[pair] != second[pair]}
    resolved = {pair: grade for pair, grade in first.items() if pair not in conflicts}
    if conflicts:
        if adjudication is None:
            raise ValueError(f"{len(conflicts)} disagreements require explicit adjudication; no automatic consensus")
        adjudicated = {}
        for row in csv_read(adjudication, ADJUDICATION):
            pair = (row["query_id"], row["doc_id"])
            if pair not in conflicts or pair in adjudicated or row["grade"] not in ("0", "1", "2", "3"):
                raise ValueError("invalid, duplicate or extra adjudication")
            if not row["note"].strip() or not row["assessor_id"].strip() or row["assessor_id"].strip() in (a, b):
                raise ValueError("disagreements require a distinct adjudicator and explanation")
            adjudicated[pair] = int(row["grade"])
        if set(adjudicated) != conflicts:
            raise ValueError("unresolved disagreements")
        resolved.update(adjudicated)
    elif adjudication is not None:
        raise ValueError("adjudication supplied when no disagreement exists")
    queries = list(records(root / "admin/queries.jsonl"))
    splits = manifest["query_splits"]
    if set(splits) != {q["_id"] for q in queries} or set(splits.values()) != {"train", "test"}:
        raise ValueError("split assignment does not match frozen queries")
    for query in splits:
        if not any(q == query and grade > 0 for (q, _), grade in resolved.items()):
            raise ValueError(f"query {query} has no judged positive; do not silently drop it from evaluation")
    destination = Path(directory)
    destination.mkdir(exist_ok=False)
    (destination / "qrels").mkdir()
    shutil.copyfile(root / "admin/corpus.jsonl", destination / "corpus.jsonl")
    shutil.copyfile(root / "admin/queries.jsonl", destination / "queries.jsonl")
    for split in ("train", "test"):
        selected = [q for q in queries if splits[q["_id"]] == split]
        (destination / f"queries-{split}.jsonl").write_text("".join(json.dumps(q, ensure_ascii=False) + "\n" for q in selected), encoding="utf-8")
        with (destination / "qrels" / f"{split}.tsv").open("w", encoding="utf-8", newline="") as output:
            writer = csv.writer(output, delimiter="\t")
            writer.writerow(["query-id", "corpus-id", "score"])
            writer.writerows((q, d, grade) for (q, d), grade in sorted(resolved.items()) if splits[q] == split)
    files = ["corpus.jsonl", "queries.jsonl", "queries-train.jsonl", "queries-test.jsonl", "qrels/train.tsv", "qrels/test.tsv"]
    receipt = {"schema": "bib-reviewed-judgments-v1", "ready_for_relevance_evaluation": True,
               "judgment_origin": "declared human; identities not independently authenticated", "declaration": declaration,
               "pool_manifest_sha256": sha256(root / "manifest.json"), "query_count": len(queries), "pair_count": len(resolved),
               "assessors": [a, b], "disagreements": len(conflicts), "agreement_fraction": 1 - len(conflicts) / len(resolved),
               "assessment_sha256": [sha256(path) for path in assessments],
               "adjudication_sha256": sha256(adjudication) if adjudication else None,
               "generated_sha256": {p: sha256(destination / p) for p in files},
               "limitations": ["Incomplete pooled judgments, not exhaustive relevance", "No relevance metric is computed by this exporter",
                               "Readiness is format/provenance completeness, not a quality or minimum sample-size acceptance claim"]}
    (destination / "manifest.json").write_text(json.dumps(receipt, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
    return receipt


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    commands = parser.add_subparsers(dest="command", required=True)
    prepare = commands.add_parser("pool")
    prepare.add_argument("--corpus", type=Path, required=True)
    prepare.add_argument("--queries", type=Path, required=True)
    prepare.add_argument("--run", action="append", required=True, metavar="NAME=PATH")
    prepare.add_argument("--directory", type=Path, required=True)
    prepare.add_argument("--top-k", type=int, default=30)
    prepare.add_argument("--seed", default="bib-judgments-v1")
    finish = commands.add_parser("export")
    finish.add_argument("--pool", type=Path, required=True)
    finish.add_argument("--assessment", action="append", type=Path, required=True)
    finish.add_argument("--adjudication", type=Path)
    finish.add_argument("--declaration", required=True)
    finish.add_argument("--directory", type=Path, required=True)
    args = parser.parse_args()
    try:
        if args.command == "pool":
            run_paths = [entry.split("=", 1) for entry in args.run]
            if any(len(entry) != 2 for entry in run_paths):
                raise ValueError("runs use NAME=PATH")
            result = pool(args.corpus, args.queries, run_paths, args.directory, args.top_k, args.seed)
        else:
            result = export(args.pool, args.assessment, args.directory, args.declaration, args.adjudication)
    except (ValueError, OSError, KeyError) as error:
        parser.error(str(error))
    print(json.dumps(result, ensure_ascii=False))


if __name__ == "__main__":
    main()
