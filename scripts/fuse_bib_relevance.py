#!/usr/bin/env python3
"""Tune weighted RRF on train only, or apply frozen parameters without tuning.

This is a diagnostic experiment, not the production hybrid implementation.
"""
import argparse
import json
from pathlib import Path

from evaluate_bib_relevance import evaluate, judgments, load_run, sha256


def fuse(lexical, dense, weight, constant, depth=100):
    combined = {}
    for query in lexical:
        scores = {}
        for ranking, multiplier in ((lexical[query], 1), (dense[query], weight)):
            for rank, doc in enumerate(ranking[:depth], 1):
                scores[doc] = scores.get(doc, 0) + multiplier / (constant + rank)
        combined[query] = sorted(scores.items(), key=lambda pair: (-pair[1], pair[0]))[:depth]
    return combined


def write_run(path, ranking):
    with path.open("x", encoding="utf-8") as output:
        for query in sorted(ranking):
            output.write(json.dumps({"query_id": query, "hits": [
                {"article_id": doc, "score": score} for doc, score in ranking[query]]}) + "\n")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("command", choices=["tune", "apply"])
    parser.add_argument("--dataset", type=Path, required=True)
    parser.add_argument("--split", choices=["train", "test"], required=True)
    parser.add_argument("--lexical", type=Path, required=True)
    parser.add_argument("--dense", type=Path, required=True)
    parser.add_argument("--parameters", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    if args.command == "tune" and args.split != "train":
        parser.error("parameter selection is allowed on train only")
    if args.output.exists() or (args.command == "tune" and args.parameters.exists()):
        parser.error("output or tuning parameter file already exists")
    judged, corpus = judgments(args.dataset, args.split)
    lexical, dense = [load_run(path, judged, corpus) for path in (args.lexical, args.dense)]
    if args.command == "tune":
        candidates = []
        for constant in (10, 30, 60):
            for weight in (0.25, 0.5, 1, 2, 4):
                combined = fuse(lexical, dense, weight, constant)
                score = evaluate({query: [doc for doc, _ in hits] for query, hits in combined.items()}, judged)["mean"]
                candidates.append({"dense_weight": weight, "rrf_constant": constant, "mean": score})
        best = max(candidates, key=lambda row: (row["mean"]["ndcg_at_10"], row["mean"]["recall_at_50"],
                                              -abs(row["dense_weight"] - 1), row["rrf_constant"]))
        parameters = {"schema": "bib-rrf-experiment-v1", "selected_on": "train", "selection_metric": "ndcg_at_10",
                      "depth": 100, "dense_weight": best["dense_weight"], "rrf_constant": best["rrf_constant"],
                      "corpus_sha256": sha256(args.dataset / "corpus.jsonl"),
                      "tuning_qrels_sha256": sha256(args.dataset / "qrels/train.tsv"),
                      "tuning_lexical_sha256": sha256(args.lexical), "tuning_dense_sha256": sha256(args.dense),
                      "grid": candidates, "production_hybrid": False}
        args.parameters.write_text(json.dumps(parameters, indent=2) + "\n", encoding="utf-8")
    else:
        parameters = json.loads(args.parameters.read_text())
        if parameters["schema"] != "bib-rrf-experiment-v1" or parameters["selected_on"] != "train" or parameters["corpus_sha256"] != sha256(args.dataset / "corpus.jsonl"):
            raise ValueError("parameters were not frozen on this corpus's train split")
        if parameters["dense_weight"] not in (0.25, 0.5, 1, 2, 4) or parameters["rrf_constant"] not in (10, 30, 60) or parameters["depth"] != 100:
            raise ValueError("invalid frozen RRF parameters")
    ranking = fuse(lexical, dense, parameters["dense_weight"], parameters["rrf_constant"], parameters["depth"])
    write_run(args.output, ranking)
    print(json.dumps({"command": args.command, "split": args.split, "dense_weight": parameters["dense_weight"],
                      "rrf_constant": parameters["rrf_constant"], "parameters_sha256": sha256(args.parameters),
                      "run_sha256": sha256(args.output), "production_hybrid": False}))


if __name__ == "__main__":
    main()
