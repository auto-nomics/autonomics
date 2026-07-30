"""Backward-compat shim — delegates to the unified connection layer.

Historical ld_score scripts used ``from config import get_catalog``. They now
get the same singleton catalog as every other infra script, via the
``datalake`` package.

If you are writing a **new** script, import directly:
    from datalake import get_catalog
"""

# Re-export for backward compatibility
from datalake.catalog import IcebergEnv, get_catalog

__all__ = ["get_catalog", "IcebergEnv"]
