"""Offline round trips for the libraries supplied to DAG Python scripts."""

import importlib
import json
import tempfile
from pathlib import Path

import duckdb
import fsspec
import pandas as pd
import polars as pl
import pyarrow as pa
import pyarrow.ipc as ipc
import sqlalchemy as sa


def main():
    for name in (
        "openpyxl", "xlsxwriter", "xlrd", "fastexcel", "s3fs", "requests",
        "httpx", "lxml.etree", "bs4", "defusedxml", "yaml", "orjson",
        "ijson", "zstandard",
    ):
        importlib.import_module(name)

    with tempfile.TemporaryDirectory(prefix="python-io-") as directory:
        root = Path(directory)
        frame = pd.DataFrame({"name": ["中文", "sample"], "value": [1, 2]})
        for suffix, separator in (("csv", ","), ("tsv", "\t")):
            path = root / f"table.{suffix}"
            frame.to_csv(path, sep=separator, index=False)
            pd.testing.assert_frame_equal(pd.read_csv(path, sep=separator), frame)
            assert pl.read_csv(path, separator=separator).to_dicts() == frame.to_dict("records")

        path = root / "table.parquet"
        frame.to_parquet(path, index=False)
        result = pl.scan_parquet(path).with_columns((pl.col("value") * 2).alias("value")).collect()
        result.write_parquet(root / "result.parquet")
        assert pd.read_parquet(root / "result.parquet")["value"].tolist() == [2, 4]

        table = pa.Table.from_pandas(frame, preserve_index=False)
        with ipc.new_file(root / "table.arrow", table.schema) as writer:
            writer.write_table(table)
        assert pl.read_ipc(root / "table.arrow").to_dicts() == frame.to_dict("records")

        path = root / "table.xlsx"
        frame.to_excel(path, index=False, engine="xlsxwriter")
        pd.testing.assert_frame_equal(pd.read_excel(path, engine="openpyxl"), frame)
        assert pl.read_excel(path, engine="calamine").to_dicts() == frame.to_dict("records")

        frame.to_json(root / "table.jsonl", orient="records", lines=True, force_ascii=False)
        assert pl.read_ndjson(root / "table.jsonl").height == 2
        frame.to_csv(root / "table.csv.gz", index=False, compression="gzip")
        pd.testing.assert_frame_equal(pd.read_csv(root / "table.csv.gz"), frame)
        with fsspec.open("memory://python-io/check.json", "wt") as handle:
            json.dump({"ok": True}, handle)
        with fsspec.open("memory://python-io/check.json") as handle:
            assert json.load(handle) == {"ok": True}

        with sa.create_engine("sqlite://").begin() as connection:
            frame.to_sql("data", connection, index=False)
            pd.testing.assert_frame_equal(pd.read_sql("SELECT * FROM data", connection), frame)
        with duckdb.connect() as connection:
            assert connection.execute("SELECT sum(value) FROM frame").fetchone() == (3,)

    print(f"Python IO smoke passed: pandas={pd.__version__}, polars={pl.__version__}, pyarrow={pa.__version__}")


if __name__ == "__main__":
    main()
