#!/usr/bin/env python3
"""Normalize checksum-pinned native Chinese CSL and NeuCLIRTech judgments.

Download only README.md + data/csl.jsonl.gz from neuclir/csl at CORPUS_REVISION,
and README.md + data/zho.tsv + data/qrels.gains.txt from neuclir/tech at
TOPICS_REVISION using hf download --type dataset --revision ... . No network,
model calls, translations, pseudo-labels or relevance scores are generated here.
All 110 official queries are acceptance queries; no Chinese tuning split exists.
"""
import argparse
import csv
import gzip
import hashlib
import json
from pathlib import Path

from evaluate_bib_relevance import judgments

CORPUS_REVISION = "69d5404fa1bb6baa80d59af2869748e05c60f8df"
TOPICS_REVISION = "a2269b2eed779694837e85c7e1c1be062d760d0b"
CHECKSUMS = {
    "corpus": {"README.md": "1e788500d017543a478897c6cb810c878010e4e68c0e2f8843a0f1e3ac8e37bb",
               "data/csl.jsonl.gz": "749c758e11b7e4d14f3e0d0a1e2cde4ac876fb155e8cdb8f72d2861398b0814e"},
    "topics": {"README.md": "a393dc0061cab20a1f8fbc77edd083f41670aeef143757888437953ecdd03570",
               "data/zho.tsv": "4bf9ac87343ab7e8618879d6ecd4f699f81d03f0691cd8392057c952337e0941",
               "data/qrels.gains.txt": "33537f361f7b00f9489768ebed813db93f0860c1ee106d3a2af1b43a651095d4"},
}


def digest(path):
    with Path(path).open("rb") as source:
        return hashlib.file_digest(source, "sha256").hexdigest()


def emit(output, row):
    output.write(json.dumps(row, ensure_ascii=False, sort_keys=True) + "\n")


def convert(corpus_source, topics_source, target):
    """Pure conversion entry point; CLI separately enforces pinned checksums."""
    target.mkdir(exist_ok=False)
    ids = set()
    with gzip.open(corpus_source / "data/csl.jsonl.gz", "rt", encoding="utf-8") as source, (target / "corpus.jsonl").open("x", encoding="utf-8") as output:
        for line in source:
            row = json.loads(line)
            key = row["doc_id"]
            if not isinstance(key, str) or not key or key in ids:
                raise ValueError("empty, nonstring or duplicate corpus ID")
            if not all(isinstance(row[field], str) for field in ["title", "abstract"]):
                raise ValueError("invalid source document text")
            ids.add(key)
            emit(output, {"_id": key, "title": row["title"], "text": row["abstract"]})
    queries = set()
    with (topics_source / "data/zho.tsv").open(encoding="utf-8", newline="") as source, (target / "queries.jsonl").open("x", encoding="utf-8") as output:
        for row in csv.reader(source, delimiter="\t"):
            if len(row) != 2 or not row[0] or not row[1].strip() or row[0] in queries:
                raise ValueError("empty or duplicate query, or invalid two-column query schema")
            queries.add(row[0])
            emit(output, {"_id": row[0], "title": "", "text": row[1]})
    if not ids or not queries:
        raise ValueError("empty corpus or queries")
    (target / "qrels").mkdir()
    pairs = set()
    with (topics_source / "data/qrels.gains.txt").open(encoding="utf-8") as source, (target / "qrels/all.tsv").open("x", encoding="utf-8", newline="") as output:
        writer = csv.writer(output, delimiter="\t", lineterminator="\n")
        writer.writerow(["query-id", "corpus-id", "score"])
        for line in source:
            columns = line.split()
            if len(columns) != 4:
                raise ValueError("invalid four-column TREC qrels schema")
            query, _iteration, document, grade = columns
            grade = int(grade)
            if query not in queries or document not in ids or grade not in (0, 1, 3):
                raise ValueError("unknown judgment object or unsupported grade")
            if (query, document) in pairs:
                raise ValueError("duplicate judgment pair")
            pairs.add((query, document))
            writer.writerow([query, document, grade])
    judged, _ = judgments(target, "all")
    if judged.keys() != queries:
        raise ValueError("all official queries must have positive judgments")
    return {"corpus_documents": len(ids), "queries": len(queries), "judgments": len(pairs)}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--corpus-source", type=Path, required=True)
    parser.add_argument("--topics-source", type=Path, required=True)
    parser.add_argument("--directory", type=Path, required=True)
    args = parser.parse_args()
    sources = {}
    for name, root, repo, revision, license_name in [
        ("corpus", args.corpus_source, "neuclir/csl", CORPUS_REVISION, "apache-2.0"),
        ("topics", args.topics_source, "neuclir/tech", TOPICS_REVISION, "cc-by-4.0"),
    ]:
        for path, expected in CHECKSUMS[name].items():
            if digest(root / path) != expected:
                raise ValueError(f"pinned {name} source checksum mismatch: {path}")
        sources[name] = {"repository": repo, "revision": revision, "declared_license": license_name, "sha256": CHECKSUMS[name]}
    counts = convert(args.corpus_source, args.topics_source, args.directory)
    if counts != {"corpus_documents": 395927, "queries": 110, "judgments": 35962}:
        raise ValueError("unexpected pinned source counts")
    manifest = {"schema": "bib-relevance-dataset-v1", "dataset": "NeuCLIRTech original Chinese CSL",
                "language": "zh", "sources": sources, "counts": counts, "splits": {"all": 110},
                "tuning_split": None, "acceptance_split": "all", "primary_metric": "ndcg_at_20",
                "query_policy": "Exact official zho.tsv second column; no translation, narrative expansion or qrels-derived terms",
                "document_policy": "Full native-Chinese corpus; doc_id/title/abstract mapped verbatim to _id/title/text",
                "ranking_policy": "Freeze English-selected production parameters before Chinese retrieval; no Chinese test tuning",
                "gain": "Unmodified published 0/1/3 gains; linear trec_eval ndcg_cut",
                "limitations": ["Incomplete published relevance pools; unjudged documents score zero",
                                "Public evaluation data may occur in model pretraining",
                                "No GUI, known-identifier or PDF evidence claim", "No quality score is computed by conversion"],
                "generated_sha256": {p: digest(args.directory / p) for p in ["corpus.jsonl", "queries.jsonl", "qrels/all.tsv"]}}
    (args.directory / "manifest.json").write_text(json.dumps(manifest, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
    print(json.dumps(manifest, ensure_ascii=False))


if __name__ == "__main__":
    main()
