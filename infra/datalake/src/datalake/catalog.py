"""Unified Iceberg REST-catalog connection layer.

This is the **single source of truth** for Iceberg connectivity across all
infra Python scripts. Every script should ``from datalake import get_catalog``
instead of maintaining its own catalog configuration.

Environment variables
---------------------
ICEBERG_REST_URI             REST catalog endpoint      (``http://localhost:8181/catalog``)
ICEBERG_S3_ENDPOINT          S3 / Garage internal       (``http://localhost:3900``)
ICEBERG_S3_ACCESS_KEY_ID     S3 access key              (**required**)
ICEBERG_S3_SECRET_ACCESS_KEY S3 secret key              (**required**)
ICEBERG_S3_REGION            S3 region                  (``garage``)
ICEBERG_S3_BUCKET            Bucket name                (``datalake``)
"""

from __future__ import annotations

import os
from dataclasses import dataclass

import polars as pl
from pyiceberg.catalog import Catalog, load_catalog


# ---------------------------------------------------------------------------
# Environment configuration
# ---------------------------------------------------------------------------


@dataclass(frozen=True)
class IcebergEnv:
    """Resolved Iceberg connection parameters from environment variables."""

    rest_uri: str
    s3_endpoint: str
    s3_access_key: str
    s3_secret_key: str
    s3_region: str
    warehouse: str

    @classmethod
    def from_env(cls) -> IcebergEnv:
        return cls(
            rest_uri=os.environ.get(
                "ICEBERG_REST_URI", "http://localhost:8181/catalog"
            ),
            s3_endpoint=os.environ.get("ICEBERG_S3_ENDPOINT", "http://localhost:3900"),
            s3_access_key=os.environ["ICEBERG_S3_ACCESS_KEY_ID"],
            s3_secret_key=os.environ["ICEBERG_S3_SECRET_ACCESS_KEY"],
            s3_region=os.environ.get("ICEBERG_S3_REGION", "garage"),
            warehouse=os.environ.get("ICEBERG_S3_BUCKET", "datalake"),
        )


def _set_aws_env(env: IcebergEnv) -> None:
    """Populate AWS_* env vars so that polars/pyarrow S3 scans work."""
    os.environ.setdefault("AWS_ACCESS_KEY_ID", env.s3_access_key)
    os.environ.setdefault("AWS_SECRET_ACCESS_KEY", env.s3_secret_key)
    os.environ.setdefault("AWS_ENDPOINT_URL", env.s3_endpoint)
    os.environ.setdefault("AWS_REGION", env.s3_region)
    os.environ.setdefault("AWS_ALLOW_HTTP", "true")


# ---------------------------------------------------------------------------
# Singleton catalog
# ---------------------------------------------------------------------------

_catalog: Catalog | None = None


def get_catalog(warehouse: str | None = None) -> Catalog:
    """Load the shared Iceberg REST catalog (singleton).

    All infra scripts should call this instead of creating their own
    ``load_catalog(...)`` — it ensures consistent connection settings and
    reuses the same catalog object across a process.

    Parameters
    ----------
    warehouse:
        Warehouse identifier inside the REST catalog.
        Defaults to the ``ICEBERG_S3_BUCKET`` env var (``"datalake"``).
    """
    global _catalog
    if _catalog is not None and warehouse is None:
        return _catalog

    env = IcebergEnv.from_env()
    _set_aws_env(env)

    _catalog = load_catalog(
        "iceberg",
        **{
            "type": "rest",
            "uri": env.rest_uri,
            "warehouse": warehouse or env.warehouse,
            "s3.endpoint": env.s3_endpoint,
            "s3.access-key-id": env.s3_access_key,
            "s3.secret-access-key": env.s3_secret_key,
            "s3.region": env.s3_region,
            "s3.force-virtual-addressing": "false",
            "s3.connect-timeout": "30",
            "s3.request-timeout": "120",
        },
    )
    return _catalog


# ---------------------------------------------------------------------------
# Convenience helpers
# ---------------------------------------------------------------------------


def list_tables(namespace: str = "ukb") -> list[tuple[str, str]]:
    """List ``(namespace, table_name)`` tuples in a namespace."""
    return [(t[0], t[-1]) for t in get_catalog().list_tables(namespace)]


def load_table(table_fqn: str):
    """Load an Iceberg table by fully-qualified name (``ns.table``)."""
    return get_catalog().load_table(table_fqn)


def scan_table(table_fqn: str) -> pl.LazyFrame:
    """Scan an Iceberg table into a polars LazyFrame."""
    return pl.scan_iceberg(load_table(table_fqn))


if __name__ == "__main__":
    for ns, name in list_tables():
        print(f"{ns}.{name}")
