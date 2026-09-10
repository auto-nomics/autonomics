from __future__ import annotations

import os
from dataclasses import dataclass


def _bool_env(name: str, default: bool) -> bool:
    value = os.getenv(name)
    if value is None:
        return default
    return value.strip().lower() in {"1", "true", "yes", "on"}


@dataclass(frozen=True)
class ServiceConfig:
    checkpoint: str = os.getenv("TIMESFM_CHECKPOINT", "google/timesfm-3.0-pytorch")
    device: str | None = os.getenv("TIMESFM_DEVICE") or None
    batch_size: int = int(os.getenv("TIMESFM_BATCH_SIZE", "4"))
    eager_load: bool = _bool_env("TIMESFM_EAGER_LOAD", False)
    local_files_only: bool = _bool_env("TIMESFM_LOCAL_FILES_ONLY", False)
    revision: str | None = os.getenv("TIMESFM_REVISION") or None


def load_config() -> ServiceConfig:
    batch_size = int(os.getenv("TIMESFM_BATCH_SIZE", "4"))
    if batch_size < 1:
        raise ValueError("TIMESFM_BATCH_SIZE must be at least 1")
    return ServiceConfig(
        checkpoint=os.getenv("TIMESFM_CHECKPOINT", "google/timesfm-3.0-pytorch"),
        device=os.getenv("TIMESFM_DEVICE") or None,
        batch_size=batch_size,
        eager_load=_bool_env("TIMESFM_EAGER_LOAD", False),
        local_files_only=_bool_env("TIMESFM_LOCAL_FILES_ONLY", False),
        revision=os.getenv("TIMESFM_REVISION") or None,
    )
