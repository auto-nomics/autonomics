"""Evaluator correctness and anti-inflation checks; run with unittest discovery."""
import json
import math
from pathlib import Path
import tempfile
import unittest

from evaluate_bib_relevance import compare, evaluate, judgments, load_run, metrics
from fuse_bib_relevance import fuse


class RelevanceTests(unittest.TestCase):
    def test_weighted_rrf_uses_ranks_and_unions_missing_candidates(self):
        result = fuse({"q": ["a", "b"]}, {"q": ["b", "c"]}, 2, 10)["q"]
        self.assertEqual([doc for doc, _ in result], ["b", "c", "a"])
        self.assertAlmostEqual(result[0][1], 1 / 12 + 2 / 11)
        self.assertAlmostEqual(result[1][1], 2 / 12)
        self.assertAlmostEqual(result[2][1], 1 / 11)

    def test_rrf_empty_results_and_ties_are_deterministic(self):
        result = fuse({"q": ["z"], "empty": []}, {"q": ["a"], "empty": []}, 1, 60)
        self.assertEqual(result["q"], [("a", 1 / 61), ("z", 1 / 61)])
        self.assertEqual(result["empty"], [])

    def test_graded_gain_matches_trec_eval_linear_definition(self):
        actual = metrics(["b", "a", "unjudged"], {"a": 3, "b": 1})
        self.assertAlmostEqual(actual["ndcg_at_10"], (1 + 3 / math.log2(3)) / (3 + 1 / math.log2(3)))
        self.assertEqual(actual["mrr_at_10"], 1)
        self.assertEqual(actual["recall_at_50"], 1)

    def test_rank_cutoffs_and_zero_results(self):
        ranking = [f"noise-{i}" for i in range(50)] + ["a"]
        actual = metrics(ranking, {"a": 1, "b": 1})
        self.assertEqual(actual["ndcg_at_10"], 0)
        self.assertEqual(actual["mrr_at_10"], 0)
        self.assertEqual(actual["recall_at_50"], 0)
        self.assertEqual(actual["recall_at_100"], 0.5)
        self.assertEqual(metrics([], {"a": 1})["zero_hits"], 1)

    def test_missing_queries_are_not_dropped_from_denominator(self):
        result = evaluate({"q1": ["a"], "q2": []}, {"q1": {"a": 1}, "q2": {"a": 1}})
        self.assertEqual(result["mean"]["ndcg_at_10"], 0.5)
        self.assertEqual(result["mean"]["zero_hits"], 0.5)

    def test_comparison_zero_baseline_has_no_infinite_relative_gain(self):
        base = evaluate({"q": []}, {"q": {"a": 1}})
        best = evaluate({"q": ["a"]}, {"q": {"a": 1}})
        result = compare(best, base)
        self.assertIsNone(result["ndcg_at_10_relative_delta"])
        self.assertEqual(result["paired_bootstrap_95_percent_interval"], [1, 1])

    def test_corrupt_and_incomplete_runs_are_rejected(self):
        judged, corpus = {"q": {"a": 1}}, {"a", "b"}
        invalid = [[], [{"query_id": "other", "hits": []}],
                   [{"query_id": "q", "hits": []}] * 2]
        for hits in [[{"article_id": "a", "score": float("nan")}],
                     [{"article_id": "unknown", "score": 1}],
                     [{"article_id": "a", "score": 1}] * 2,
                     [{"article_id": "a", "score": 1}, {"article_id": "b", "score": 2}]]:
            invalid.append([{"query_id": "q", "hits": hits}])
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "run.jsonl"
            for rows in invalid:
                path.write_text("".join(json.dumps(row) + "\n" for row in rows))
                with self.subTest(rows=rows), self.assertRaises(ValueError):
                    load_run(path, judged, corpus)
            path.write_text('{"query_id":"q","hits":[]}\n')
            self.assertEqual(load_run(path, judged, corpus), {"q": []})

    def test_tuning_acceptance_overlap_and_invalid_judgments_are_rejected(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / "qrels").mkdir()
            for name in ("corpus", "queries"):
                (root / f"{name}.jsonl").write_text('{"_id":"a"}\n{"_id":"b"}\n')
            header = "query-id\tcorpus-id\tscore\n"
            (root / "qrels/train.tsv").write_text(header + "a\ta\t1\n")
            test = root / "qrels/test.tsv"
            for row in ["a\tb\t1\n", "b\tunknown\t1\n", "b\tb\t-1\n", "b\tb\t0\n", "b\tb\t1\nb\tb\t1\n"]:
                test.write_text(header + row)
                with self.subTest(row=row), self.assertRaises(ValueError):
                    judgments(root, "test")
            test.write_text(header + "b\tb\t1\n")
            self.assertEqual(judgments(root, "test")[0], {"b": {"b": 1}})


if __name__ == "__main__":
    unittest.main()
