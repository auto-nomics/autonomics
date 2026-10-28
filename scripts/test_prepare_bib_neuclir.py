import gzip
import json
import math
from pathlib import Path
import tempfile
import unittest

from evaluate_bib_relevance import compare, evaluate, judgments, metrics
from prepare_bib_neuclir import convert


class NeuclirConversionTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.addCleanup(self.temporary.cleanup)
        self.root = Path(self.temporary.name)
        self.source = self.root / "source"
        (self.source / "data").mkdir(parents=True)
        self.documents = [{"doc_id": "a", "title": "文献甲（格式测试）", "abstract": "完整的原始摘要。"},
                          {"doc_id": "b", "title": "文献乙（格式测试）", "abstract": "另一项研究。"}]
        self.write_documents()
        (self.source / "data/zho.tsv").write_text("1\t原始中文查询\n2\t另一条查询\n", encoding="utf-8")
        self.qrels = self.source / "data/qrels.gains.txt"
        self.qrels.write_text("1 0 a 3\n1 0 b 0\n2 0 b 1\n", encoding="utf-8")
        self.target = self.root / "out"

    def write_documents(self):
        with gzip.open(self.source / "data/csl.jsonl.gz", "wt", encoding="utf-8") as output:
            for row in self.documents:
                output.write(json.dumps(row, ensure_ascii=False) + "\n")

    def test_native_text_ids_and_all_official_gains_survive_without_fake_split(self):
        self.assertEqual(convert(self.source, self.source, self.target), {"corpus_documents": 2, "queries": 2, "judgments": 3})
        rows = [json.loads(line) for line in (self.target / "corpus.jsonl").read_text().splitlines()]
        self.assertEqual(rows[0], {"_id": "a", "title": self.documents[0]["title"], "text": self.documents[0]["abstract"]})
        self.assertEqual(judgments(self.target, "all")[0], {"1": {"a": 3, "b": 0}, "2": {"b": 1}})
        queries = (self.target / "queries.jsonl").read_text()
        self.assertIn("原始中文查询", queries)
        self.assertNotIn("score", queries)
        self.assertFalse((self.target / "qrels/train.tsv").exists())

    def test_unknown_document_and_duplicate_judgment_are_rejected(self):
        for index, text in enumerate(["1 0 unknown 3\n", "1 0 a 3\n1 0 a 3\n"]):
            self.qrels.write_text(text)
            with self.assertRaises(ValueError):
                convert(self.source, self.source, self.root / f"bad{index}")

    def test_missing_positive_or_unjudged_query_cannot_be_silently_dropped(self):
        for index, text in enumerate(["1 0 a 0\n2 0 b 1\n", "1 0 a 3\n"]):
            self.qrels.write_text(text)
            with self.assertRaises(ValueError):
                convert(self.source, self.source, self.root / f"bad{index}")

    def test_source_duplicate_ids_and_unsupported_gains_fail(self):
        self.documents.append(self.documents[0])
        self.write_documents()
        with self.assertRaisesRegex(ValueError, "duplicate corpus"):
            convert(self.source, self.source, self.target)
        self.documents.pop()
        self.write_documents()
        self.qrels.write_text("1 0 a 2\n2 0 b 1\n")
        with self.assertRaisesRegex(ValueError, "unsupported grade"):
            convert(self.source, self.source, self.root / "grade")

    def test_ndcg_twenty_includes_rank_eleven_with_linear_gain(self):
        result = metrics([str(i) for i in range(10)] + ["relevant"], {"relevant": 3})
        self.assertEqual(result["ndcg_at_10"], 0)
        self.assertAlmostEqual(result["ndcg_at_20"], 1 / math.log2(12))
        current = evaluate({"q": [str(i) for i in range(10)] + ["relevant"]}, {"q": {"relevant": 3}})
        baseline = evaluate({"q": []}, {"q": {"relevant": 3}})
        self.assertEqual(compare(current, baseline)["ndcg_at_10_absolute_delta"], 0)
        self.assertGreater(compare(current, baseline, 20)["ndcg_at_20_absolute_delta"], 0)


if __name__ == "__main__":
    unittest.main()
