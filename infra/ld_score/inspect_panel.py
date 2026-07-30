"""Iceberg catalog helper for ad-hoc inspection.

Usage
-----
    python datalake.py                      # smoke-test: load catalog, print gwas.test
    python -c "from datalake import Datalake; print(Datalake().list_all())"
"""

from __future__ import annotations

from config import get_catalog


class Datalake:
    """Thin wrapper around the shared catalog configuration."""

    def get_catalog(self):
        return get_catalog()

    def preview_table(self, table_ident: str):
        """Load an Iceberg table by its fully-qualified identifier."""
        catalog = self.get_catalog()
        return catalog.load_table(table_ident)

    def list_namespaces(self):
        catalog = self.get_catalog()
        return catalog.list_namespaces()

    def list_tables(self, namespace: str):
        """List tables under a single namespace level."""
        catalog = self.get_catalog()
        return catalog.list_tables(namespace)

    def list_all(self):
        """Recursively list all tables across every namespace."""
        catalog = self.get_catalog()
        tables = []
        for ns in catalog.list_namespaces():
            ns_name = ns if isinstance(ns, str) else ns[0]
            try:
                for t in catalog.list_tables(ns_name):
                    tables.append(f"{ns_name}.{t}" if isinstance(ns, tuple) else t)
            except Exception:
                pass
        return tables


# ---------------------------------------------------------------------------
# Smoke-test on import
# ---------------------------------------------------------------------------
if __name__ == "__main__":
    dl = Datalake()
    print("Namespaces:", dl.list_namespaces())
    try:
        tb = dl.preview_table("gwas.test")
        print("gwas.test:", tb)
    except Exception as e:
        print(f"gwas.test not found or catalog unavailable: {e}")
