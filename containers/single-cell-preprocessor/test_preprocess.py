"""Unit tests for preprocess.py boundary cases and diagnostics."""

from __future__ import annotations

import os
import sys
import tempfile
from pathlib import Path

import pytest

sys.path.insert(0, str(Path(__file__).parent))
from preprocess import (
    ContractError,
    _check_exists,
    _detect_separator,
    parse_matrix_header,
    read_barcodes,
    read_features,
    read_metadata,
)


def _write(path: Path, content: str) -> Path:
    path.write_text(content, encoding="ascii")
    return path


class TestCheckExists:
    def test_missing_file(self, tmp_path: Path) -> None:
        with pytest.raises(ContractError, match="does not exist"):
            _check_exists(tmp_path / "nope.tsv", "barcode")

    def test_empty_file(self, tmp_path: Path) -> None:
        p = tmp_path / "empty.tsv"
        p.write_text("")
        with pytest.raises(ContractError, match="0 bytes"):
            _check_exists(p, "barcode")

    def test_directory_is_rejected(self, tmp_path: Path) -> None:
        with pytest.raises(ContractError, match="not a regular file"):
            _check_exists(tmp_path, "matrix")


class TestDetectSeparator:
    def test_tab(self, tmp_path: Path) -> None:
        p = _write(tmp_path / "meta.tsv", "cell_id\tdonor\n")
        os.environ.pop("AUTONOMICS_SINGLE_CELL_METADATA_SEP", None)
        assert _detect_separator(p) == "\t"

    def test_comma(self, tmp_path: Path) -> None:
        p = _write(tmp_path / "meta.csv", "cell_id,donor\n")
        os.environ.pop("AUTONOMICS_SINGLE_CELL_METADATA_SEP", None)
        assert _detect_separator(p) == ","

    def test_explicit_tab(self, tmp_path: Path) -> None:
        p = _write(tmp_path / "meta.csv", "cell_id,donor\n")
        os.environ["AUTONOMICS_SINGLE_CELL_METADATA_SEP"] = "tab"
        assert _detect_separator(p) == "\t"

    def test_explicit_comma(self, tmp_path: Path) -> None:
        p = _write(tmp_path / "meta.tsv", "cell_id\tdonor\n")
        os.environ["AUTONOMICS_SINGLE_CELL_METADATA_SEP"] = "comma"
        assert _detect_separator(p) == ","


class TestMatrixHeader:
    def test_minimal_1x1(self, tmp_path: Path) -> None:
        p = _write(tmp_path / "m.mtx", "%%MatrixMarket matrix coordinate integer general\n1 1 1\n1 1 1\n")
        h = parse_matrix_header(p)
        assert h.rows == 1
        assert h.columns == 1
        assert h.nonzero == 1

    def test_full_fill(self, tmp_path: Path) -> None:
        p = _write(
            tmp_path / "m.mtx",
            "%%MatrixMarket matrix coordinate integer general\n2 2 4\n1 1 1\n1 2 2\n2 1 3\n2 2 4\n",
        )
        h = parse_matrix_header(p)
        assert h.nonzero == h.rows * h.columns

    def test_rejects_non_matrix(self, tmp_path: Path) -> None:
        p = _write(tmp_path / "bad.txt", "not a matrix\n")
        with pytest.raises(ContractError, match="not a MatrixMarket"):
            parse_matrix_header(p)


class TestReadMetadata:
    def _barcodes(self, ids: list[str]) -> object:
        import pandas as pd
        return pd.DataFrame({"cell_barcode": ids})

    def test_tab_separated(self, tmp_path: Path) -> None:
        import pandas as pd
        p = _write(tmp_path / "meta.tsv", "cell_id\tdonor\nA\tx\nB\ty\n")
        os.environ.pop("AUTONOMICS_SINGLE_CELL_METADATA_SEP", None)
        frame, _ = read_metadata(p, self._barcodes(["A", "B"]))
        assert list(frame.index) == ["A", "B"]
        assert list(frame["donor"]) == ["x", "y"]

    def test_comma_separated_auto(self, tmp_path: Path) -> None:
        p = _write(tmp_path / "meta.csv", "cell_id,donor\nA,x\nB,y\n")
        os.environ.pop("AUTONOMICS_SINGLE_CELL_METADATA_SEP", None)
        frame, _ = read_metadata(p, self._barcodes(["A", "B"]))
        assert list(frame.index) == ["A", "B"]

    def test_duplicate_ids(self, tmp_path: Path) -> None:
        p = _write(tmp_path / "meta.tsv", "cell_id\tdonor\nA\tx\nA\ty\n")
        os.environ.pop("AUTONOMICS_SINGLE_CELL_METADATA_SEP", None)
        with pytest.raises(ContractError, match="duplicate cell IDs"):
            read_metadata(p, self._barcodes(["A", "A"]))

    def test_mismatched_ids(self, tmp_path: Path) -> None:
        p = _write(tmp_path / "meta.tsv", "cell_id\tdonor\nA\tx\n")
        os.environ.pop("AUTONOMICS_SINGLE_CELL_METADATA_SEP", None)
        with pytest.raises(ContractError, match="do not align"):
            read_metadata(p, self._barcodes(["A", "B"]))


class TestDimensionMismatch:
    def test_barcode_count_mismatch(self, tmp_path: Path) -> None:
        import pandas as pd
        matrix = _write(tmp_path / "m.mtx", "%%MatrixMarket matrix coordinate integer general\n2 3 2\n1 1 1\n2 2 1\n")
        barcodes = _write(tmp_path / "bc.tsv", "A\nB\n")
        features = _write(tmp_path / "ft.tsv", "g1\tsym1\ttype\ng2\tsym2\ttype\ng3\tsym3\ttype\n")
        h = parse_matrix_header(matrix)
        bc = read_barcodes(barcodes)
        ft = read_features(features)
        assert h.columns != len(bc)

    def test_feature_count_mismatch(self, tmp_path: Path) -> None:
        matrix = _write(tmp_path / "m.mtx", "%%MatrixMarket matrix coordinate integer general\n5 3 2\n1 1 1\n2 2 1\n")
        barcodes = _write(tmp_path / "bc.tsv", "A\nB\nC\n")
        features = _write(tmp_path / "ft.tsv", "g1\tsym1\ttype\ng2\tsym2\ttype\n")
        h = parse_matrix_header(matrix)
        bc = read_barcodes(barcodes)
        ft = read_features(features)
        assert h.rows != len(ft)
