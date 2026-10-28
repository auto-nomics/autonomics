#!/usr/bin/env python3
"""Local Ollama exact-cosine diagnostic; does not implement production hybrid search.

Requires numpy. Consumes corpus.jsonl and a qrels-free query JSONL. Persistent
SQLite cache binds exact input bytes, model digest, and inference options.
Only loopback services are accepted; no private library data is loaded.
"""
import argparse
import hashlib
import ipaddress
import json
from pathlib import Path
import sqlite3
import time
import urllib.parse
import urllib.request

import numpy as np

from evaluate_bib_relevance import load_run, records, sha256


def request(base, path, data=None):
    req = urllib.request.Request(base + path, data=None if data is None else json.dumps(data).encode(),
                                 headers={"Content-Type": "application/json"})
    with urllib.request.urlopen(req, timeout=180) as response:
        body = response.read(32 * 1024 * 1024 + 1)
    if len(body) > 32 * 1024 * 1024:
        raise ValueError("Ollama response exceeds 32 MiB")
    return json.loads(body)


def verify_model(base, model, digest):
    found = [row["digest"] for row in request(base, "/api/tags")["models"] if row["name"] == model]
    if found != [digest]:
        raise ValueError("model alias is missing or its digest changed")


def chunks(title, text, size, overlap):
    if not size:
        return [title + "\n" + text]
    if not text:
        return [title]
    # Code point windows, not token windows; report this explicitly in receipts.
    result = []
    start = 0
    while start < len(text):
        end = min(start + size, len(text))
        result.append(title + "\n" + text[start:end])
        if end == len(text):
            break
        start = end - overlap
    return result


def embed(base, model, digest, texts, connection, batch_size, stats, options):
    outputs = []
    profile_data = {"digest": digest, "truncate": False, "api": "ollama/api/embed-v1"}
    if options:
        profile_data["options"] = options
    profile = json.dumps(profile_data, sort_keys=True)
    for start in range(0, len(texts), batch_size):
        batch = texts[start:start + batch_size]
        keys = [hashlib.sha256((profile + "\0" + text).encode()).hexdigest() for text in batch]
        values = {}
        for key in dict.fromkeys(keys):
            row = connection.execute("SELECT dimensions, vector FROM embeddings WHERE key=?", (key,)).fetchone()
            if row:
                vector = np.frombuffer(row[1], dtype="<f4").copy()
                if len(vector) != row[0] or not np.isfinite(vector).all() or abs(float(np.linalg.norm(vector)) - 1) > 1e-4:
                    raise ValueError("corrupt cached embedding")
                values[key] = vector
        pending = list(dict.fromkeys(key for key in keys if key not in values))
        if pending:
            by_key = dict(zip(keys, batch))
            verify_model(base, model, digest)
            started = time.perf_counter()
            body = {"model": model, "input": [by_key[key] for key in pending], "truncate": False}
            if options:
                body["options"] = options
            payload = request(base, "/api/embed", body)
            stats["embedding_service_seconds"] += time.perf_counter() - started
            verify_model(base, model, digest)
            raw = payload["embeddings"]
            if len(raw) != len(pending):
                raise ValueError("embedding count does not match request")
            for key, vector in zip(pending, raw):
                vector = np.asarray(vector, dtype="<f4")
                if vector.ndim != 1 or not vector.size or not np.isfinite(vector).all():
                    raise ValueError("invalid embedding")
                norm = float(np.linalg.norm(vector))
                if not np.isfinite(norm) or norm <= 1e-12:
                    raise ValueError("zero or invalid embedding norm")
                vector /= norm
                values[key] = vector
                connection.execute("INSERT INTO embeddings VALUES (?, ?, ?)", (key, len(vector), vector.tobytes()))
            connection.commit()
            stats["new_embeddings"] += len(pending)
        stats["cached_embeddings"] += len(keys) - len(pending)
        outputs.extend(values[key] for key in keys)
        if (start // batch_size) % 10 == 0:
            print(json.dumps({"embedded": min(start + batch_size, len(texts)), "total": len(texts), **stats}), flush=True)
    return np.stack(outputs)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--dataset", type=Path, required=True)
    parser.add_argument("--queries", type=Path, required=True)
    parser.add_argument("--model", required=True, help="Use an independent digest-named alias, not latest")
    parser.add_argument("--digest", required=True)
    parser.add_argument("--base-url", default="http://127.0.0.1:11434")
    parser.add_argument("--cache", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--batch-size", type=int, default=16)
    parser.add_argument("--chunk-chars", type=int, default=0)
    parser.add_argument("--overlap-chars", type=int, default=200)
    parser.add_argument("--context-tokens", type=int)
    parser.add_argument("--batch-tokens", type=int)
    parser.add_argument("--rerank-run", type=Path, help="Rerank an existing run's shortlist with this bi-encoder's cosine")
    parser.add_argument("--rerank-depth", type=int, default=20)
    parser.add_argument("--query-instruction", default="Given a scientific claim, retrieve scientific papers that provide relevant evidence.")
    args = parser.parse_args()
    url = urllib.parse.urlparse(args.base_url)
    local = url.hostname == "localhost"
    if not local:
        try:
            local = ipaddress.ip_address(url.hostname).is_loopback
        except ValueError:
            pass
    if not local or url.scheme not in ("http", "https") or url.username or url.password or url.query or url.fragment or url.path not in ("", "/"):
        parser.error("embedding benchmark requires a loopback URL without credentials or path")
    if not 1 <= args.batch_size <= 64 or args.chunk_chars < 0 or args.overlap_chars < 0 or (args.chunk_chars and args.overlap_chars >= args.chunk_chars):
        parser.error("invalid batch size or chunk overlap")
    if len(args.digest) != 64 or any(c not in "0123456789abcdef" for c in args.digest):
        parser.error("digest must be a SHA256 hex string")
    if any(value is not None and not 1 <= value <= 32768 for value in (args.context_tokens, args.batch_tokens)):
        parser.error("context and token batch size must be between 1 and 32768")
    if args.output.exists() or args.output.with_suffix(".receipt.json").exists():
        parser.error("output exists; use a new run filename")
    base = args.base_url.rstrip("/")
    verify_model(base, args.model, args.digest)
    corpus = list(records(args.dataset / "corpus.jsonl"))
    queries = list(records(args.queries))
    if not 1 <= args.rerank_depth <= 100:
        parser.error("rerank depth must be between 1 and 100")
    rerank = load_run(args.rerank_run, {row["_id"]: {} for row in queries}, {row["_id"] for row in corpus}) if args.rerank_run else None
    texts, owners = [], []
    for index, row in enumerate(corpus):
        parts = chunks(row["title"], row["text"], args.chunk_chars, args.overlap_chars)
        texts.extend(parts)
        owners.extend([index] * len(parts))
    query_texts = [(f"Instruct: {args.query_instruction}\nQuery: " if args.query_instruction else "") + row["text"] for row in queries]
    stats = {"embedding_service_seconds": 0, "new_embeddings": 0, "cached_embeddings": 0}
    options = {key: value for key, value in (("num_ctx", args.context_tokens), ("num_batch", args.batch_tokens)) if value is not None}
    connection = sqlite3.connect(args.cache)
    connection.execute("CREATE TABLE IF NOT EXISTS embeddings (key TEXT PRIMARY KEY, dimensions INTEGER NOT NULL, vector BLOB NOT NULL)")
    started = time.perf_counter()
    docs = embed(base, args.model, args.digest, texts, connection, args.batch_size, stats, options)
    query_vectors = embed(base, args.model, args.digest, query_texts, connection, args.batch_size, stats, options)
    connection.close()
    if docs.shape[1] != query_vectors.shape[1]:
        raise ValueError("corpus and query embedding dimensions differ")
    embed_seconds = time.perf_counter() - started
    owner_array = np.asarray(owners)
    lexical_ids = [row["_id"] for row in corpus]
    positions = {doc: index for index, doc in enumerate(lexical_ids)}
    tie_order = np.argsort(lexical_ids, kind="stable")
    latencies = []
    with args.output.open("x", encoding="utf-8") as output:
        for query, vector in zip(queries, query_vectors):
            started = time.perf_counter()
            scores = np.full(len(corpus), -np.inf, dtype=np.float32)
            if rerank is None:
                np.maximum.at(scores, owner_array, docs @ vector)
                order = tie_order[np.argsort(-scores[tie_order], kind="stable")[:100]]
            else:
                original = [positions[doc] for doc in rerank[query["_id"]]]
                shortlist = original[:args.rerank_depth]
                mask = np.isin(owner_array, shortlist)
                np.maximum.at(scores, owner_array[mask], docs[mask] @ vector)
                order = sorted(shortlist, key=lambda index: (-float(scores[index]), lexical_ids[index])) + original[args.rerank_depth:]
            latencies.append((time.perf_counter() - started) * 1000)
            output.write(json.dumps({"query_id": query["_id"], "hits": [
                {"article_id": lexical_ids[index], "score": float(scores[index]) if rerank is None else 1 / rank}
                for rank, index in enumerate(order, 1)]}) + "\n")
    verify_model(base, args.model, args.digest)
    receipt = {"schema": "bib-dense-diagnostic-v1", "production_hybrid": False, "model_digest": args.digest,
               "model_alias": args.model, "dimensions": docs.shape[1], "documents": len(corpus), "chunks": len(texts),
               "queries": len(queries), "chunk_codepoints": args.chunk_chars, "overlap_codepoints": args.overlap_chars if args.chunk_chars else 0,
               "query_instruction": args.query_instruction, "truncate": False, "inference_options": options,
               "aggregation": "max chunk cosine per article",
               "rerank_input_sha256": sha256(args.rerank_run) if rerank is not None else None,
               "rerank_depth": args.rerank_depth if rerank is not None else None,
               "rerank_method": "same bi-encoder exact cosine; not a cross-encoder" if rerank is not None else None,
               "run_score": "cosine" if rerank is None else "reciprocal final rank (shortlist cosine order, tail unchanged)",
               "embedding_wall_seconds": embed_seconds, "cosine_p50_ms": float(np.percentile(latencies, 50)),
               "cosine_p95_ms": float(np.percentile(latencies, 95)), "matrix_bytes": docs.nbytes,
               "privacy": "public benchmark texts to loopback Ollama only; cache contains derived vectors",
               "corpus_sha256": sha256(args.dataset / "corpus.jsonl"), "queries_sha256": sha256(args.queries),
               "run_sha256": sha256(args.output), "numpy_version": np.__version__, **stats}
    args.output.with_suffix(".receipt.json").write_text(json.dumps(receipt, indent=2) + "\n")
    print(json.dumps(receipt), flush=True)


if __name__ == "__main__":
    main()
