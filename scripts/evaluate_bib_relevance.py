#!/usr/bin/env python3
"""Strict BEIR-format evaluation. No model, retrieval, or relevance judgments invented here.

Runs are JSONL: {"query_id": "1", "hits": [{"article_id": "42", "score": 1.0}]}.
Every judged query must have a row, including queries with no retrieved hits.
"""
import argparse
import csv
import hashlib
import json
import math
from pathlib import Path
import random
import statistics


def sha256(path):
    return hashlib.sha256(Path(path).read_bytes()).hexdigest()


def records(path):
    seen = set()
    with Path(path).open(encoding="utf-8") as source:
        for line in source:
            row = json.loads(line)
            key = row["_id"]
            if not isinstance(key, str) or not key or key in seen:
                raise ValueError(f"invalid or duplicate ID in {path}")
            seen.add(key)
            yield row


def judgments(directory, split):
    directory = Path(directory)
    corpus = {row["_id"] for row in records(directory / "corpus.jsonl")}
    queries = {row["_id"] for row in records(directory / "queries.jsonl")}
    all_splits = {}
    for name in (("all",) if split == "all" else ("train", "test")):
        data = {}
        with (directory / "qrels" / f"{name}.tsv").open(encoding="utf-8") as source:
            rows = csv.DictReader(source, delimiter="\t")
            if rows.fieldnames != ["query-id", "corpus-id", "score"]:
                raise ValueError("unexpected qrels header")
            for row in rows:
                query, doc, grade = row["query-id"], row["corpus-id"], int(row["score"])
                if query not in queries or doc not in corpus or grade < 0:
                    raise ValueError("qrels reference unknown objects or negative grades")
                if doc in data.setdefault(query, {}):
                    raise ValueError("duplicate query/document judgment")
                data[query][doc] = grade
        if not data or any(not any(g > 0 for g in docs.values()) for docs in data.values()):
            raise ValueError("each judged query must have at least one relevant document")
        all_splits[name] = data
    if split != "all" and all_splits["train"].keys() & all_splits["test"].keys():
        raise ValueError("tuning and acceptance query IDs overlap")
    return all_splits[split], corpus


def load_run(path, judged, corpus):
    data = {}
    with Path(path).open(encoding="utf-8") as source:
        for line in source:
            row = json.loads(line)
            query = row["query_id"]
            if query not in judged or query in data:
                raise ValueError("run contains an extra or duplicate query")
            docs = []
            previous = math.inf
            for hit in row["hits"]:
                doc, score = hit["article_id"], hit["score"]
                if isinstance(score, bool) or not isinstance(score, (int, float)) or not math.isfinite(score):
                    raise ValueError("run score is not finite")
                if doc not in corpus or doc in docs or score > previous:
                    raise ValueError("run has unknown/duplicate documents or unsorted scores")
                previous = score
                docs.append(doc)
            data[query] = docs
    if data.keys() != judged.keys():
        raise ValueError("run must explicitly include all judged queries, including empty results")
    return data


def metrics(ranking, grades):
    def dcg(values):
        # trec_eval/BEIR use the qrels grade itself as gain (not 2**grade-1).
        return sum(grade / math.log2(rank + 2) for rank, grade in enumerate(values))
    ideal = dcg(sorted(grades.values(), reverse=True)[:10])
    ideal20 = dcg(sorted(grades.values(), reverse=True)[:20])
    relevant = {doc for doc, grade in grades.items() if grade > 0}
    return {
        "ndcg_at_10": dcg([grades.get(doc, 0) for doc in ranking[:10]]) / ideal,
        "ndcg_at_20": dcg([grades.get(doc, 0) for doc in ranking[:20]]) / ideal20,
        "mrr_at_10": next((1 / rank for rank, doc in enumerate(ranking[:10], 1) if doc in relevant), 0),
        "recall_at_50": len(set(ranking[:50]) & relevant) / len(relevant),
        "recall_at_100": len(set(ranking[:100]) & relevant) / len(relevant),
        "zero_hits": int(not ranking),
    }


def evaluate(run, judged):
    per_query = {query: metrics(run[query], judged[query]) for query in sorted(judged)}
    mean = {metric: statistics.mean(row[metric] for row in per_query.values())
            for metric in next(iter(per_query.values()))}
    return {"mean": mean, "per_query": per_query}


def compare(current, baseline, cutoff=10):
    metric = f"ndcg_at_{cutoff}"
    deltas = [current["per_query"][q][metric] - baseline["per_query"][q][metric]
              for q in current["per_query"]]
    rng = random.Random(20261028)
    bootstrap = sorted(statistics.mean(rng.choices(deltas, k=len(deltas))) for _ in range(2000))
    base = baseline["mean"][metric]
    return {f"{metric}_absolute_delta": statistics.mean(deltas),
            f"{metric}_relative_delta": statistics.mean(deltas) / base if base else None,
            "paired_bootstrap_95_percent_interval": [bootstrap[49], bootstrap[1949]],
            "improved_queries": sum(x > 0 for x in deltas),
            "regressed_queries": sum(x < 0 for x in deltas),
            "unchanged_queries": sum(x == 0 for x in deltas),
            "bootstrap_seed": 20261028, "bootstrap_samples": 2000}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--dataset", type=Path, required=True)
    parser.add_argument("--split", choices=["train", "test", "all"], required=True)
    parser.add_argument("--comparison-cutoff", type=int, choices=[10, 20], default=10)
    parser.add_argument("--run", type=Path, required=True)
    parser.add_argument("--baseline", type=Path)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    judged, corpus = judgments(args.dataset, args.split)
    result = evaluate(load_run(args.run, judged, corpus), judged)
    result.update({"schema": "bib-relevance-v1", "split": args.split, "queries": len(judged),
                   "corpus_documents": len(corpus), "run_sha256": sha256(args.run),
                   "dataset_sha256": {str(path.relative_to(args.dataset)): sha256(path) for path in
                       [args.dataset / "corpus.jsonl", args.dataset / "queries.jsonl",
                        *[args.dataset / "qrels" / f"{name}.tsv" for name in
                          (("all",) if args.split == "all" else ("train", "test"))]]},
                   "relevance_scope": "relative to published qrels; unjudged documents count as nonrelevant",
                   "gain": "linear qrels grade (trec_eval ndcg_cut)",
                   "ties": "explicit run order; equal numeric scores are not reordered"})
    if args.baseline:
        baseline = evaluate(load_run(args.baseline, judged, corpus), judged)
        result["baseline"] = {"sha256": sha256(args.baseline), "mean": baseline["mean"]}
        result["comparison"] = compare(result, baseline, args.comparison_cutoff)
    args.output.write_text(json.dumps(result, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
    print(json.dumps({key: value for key, value in result.items() if key != "per_query"}, ensure_ascii=False))


if __name__ == "__main__":
    main()
