import csv
import json
from pathlib import Path
import tempfile
import unittest

from evaluate_bib_relevance import judgments
from prepare_bib_judgments import ADJUDICATION, FIELDS, csv_read, csv_write, export, pool


class JudgmentWorkflowTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.addCleanup(self.temporary.cleanup)
        self.root = Path(self.temporary.name)
        self.corpus = self.root / "corpus.jsonl"
        self.queries = self.root / "queries.jsonl"
        self.write_jsonl(self.corpus, [{"_id": "a", "title": "文献甲（测试夹具）", "text": "机制A"},
                                        {"_id": "b", "title": "文献乙（测试夹具）", "text": "机制B"}])
        self.write_jsonl(self.queries, [{"_id": "train", "text": "机制A的研究", "split": "train"},
                                         {"_id": "test", "text": "机制A的独立检索任务", "split": "test"}])
        self.run = self.root / "run.jsonl"
        self.write_jsonl(self.run, [{"query_id": q, "hits": [{"article_id": "a", "score": 2}, {"article_id": "b", "score": 1}]} for q in ["train", "test"]])
        self.bundle = self.root / "pool"

    @staticmethod
    def write_jsonl(path, rows):
        path.write_text("".join(json.dumps(row, ensure_ascii=False) + "\n" for row in rows), encoding="utf-8")

    def prepare(self):
        return pool(self.corpus, self.queries, [("secret-engine-1", self.run), ("secret-engine-2", self.run)], self.bundle)

    def grades(self, name, change=None):
        rows = csv_read(self.bundle / "review/tasks.csv", FIELDS)
        for row in rows:
            row.update(grade="3" if row["doc_id"] == "a" else "0", note="格式测试夹具，非真实判断", assessor_id=name)
        if change:
            change(rows)
        path = self.root / f"{name}.csv"
        csv_write(path, FIELDS, rows)
        return path

    def test_pool_is_blind_has_no_grades_and_exports_only_reviewed_consensus(self):
        manifest = self.prepare()
        self.assertFalse(manifest["ready_for_relevance_evaluation"])
        self.assertEqual(manifest["pair_count"], 4)
        review = (self.bundle / "review/tasks.csv").read_text()
        self.assertNotIn("secret-engine", review)
        self.assertTrue(all(not row["grade"] for row in csv_read(self.bundle / "review/tasks.csv", FIELDS)))
        first, second = self.grades("reviewer-1"), self.grades("reviewer-2")
        target = self.root / "ready"
        result = export(self.bundle, [first, second], target, "Unit-test fixture only, not real human research judgments")
        self.assertEqual(result["agreement_fraction"], 1)
        self.assertEqual(judgments(target, "test")[0], {"test": {"a": 3, "b": 0}})
        with (target / "qrels/train.tsv").open(encoding="utf-8") as source:
            self.assertEqual(len(list(csv.DictReader(source, delimiter="\t"))), 2)

    def test_blanks_are_not_zero_and_one_assessor_cannot_impersonate_two(self):
        self.prepare()
        first = self.grades("one")
        second = self.grades("two", lambda rows: rows[0].update(grade=""))
        with self.assertRaisesRegex(ValueError, "explicit grade"):
            export(self.bundle, [first, second], self.root / "invalid", "test")
        with self.assertRaisesRegex(ValueError, "distinct"):
            export(self.bundle, [first, first], self.root / "invalid", "test")

    def test_disagreement_requires_distinct_explicit_adjudication(self):
        self.prepare()
        first = self.grades("one")
        second = self.grades("two", lambda rows: rows[0].update(grade="2" if rows[0]["doc_id"] == "a" else "1"))
        with self.assertRaisesRegex(ValueError, "disagreements"):
            export(self.bundle, [first, second], self.root / "invalid", "test")
        row = csv_read(first, FIELDS)[0]
        adjudication = self.root / "adjudication.csv"
        csv_write(adjudication, ADJUDICATION, [{key: row[key] for key in ADJUDICATION} | {"assessor_id": "third", "note": "format test adjudication"}])
        result = export(self.bundle, [first, second], self.root / "resolved", "test fixture", adjudication)
        self.assertEqual(result["disagreements"], 1)

    def test_group_leakage_and_changed_frozen_splits_are_rejected(self):
        self.write_jsonl(self.queries, [{"_id": "train", "text": "a", "group_id": "same", "split": "train"},
                                         {"_id": "test", "text": "b", "group_id": "same", "split": "test"}])
        with self.assertRaisesRegex(ValueError, "crosses"):
            self.prepare()

    def test_changed_frozen_splits_are_rejected(self):
        self.prepare()
        manifest_path = self.bundle / "manifest.json"
        manifest = json.loads(manifest_path.read_text())
        manifest["query_splits"] = {"train": "test", "test": "train"}
        manifest_path.write_text(json.dumps(manifest))
        with self.assertRaisesRegex(ValueError, "changed after freezing"):
            export(self.bundle, [self.grades("one"), self.grades("two")], self.root / "invalid", "test")

    def test_no_positive_queries_and_duplicate_review_pairs_are_not_silently_dropped(self):
        self.prepare()
        def all_zero(rows):
            for row in rows:
                row["grade"] = "0"
        with self.assertRaisesRegex(ValueError, "no judged positive"):
            export(self.bundle, [self.grades("one", all_zero), self.grades("two", all_zero)], self.root / "invalid", "test")
        first = self.grades("one")
        second = self.grades("two", lambda rows: rows.append(dict(rows[0])))
        with self.assertRaisesRegex(ValueError, "duplicate"):
            export(self.bundle, [first, second], self.root / "invalid", "test")


if __name__ == "__main__":
    unittest.main()
