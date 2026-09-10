from __future__ import annotations

import threading

import numpy as np

from .config import ServiceConfig


class TimesFMPool:
    """Loads the model once and serializes access from worker threads."""

    def __init__(self, config: ServiceConfig):
        self.config = config
        self._model = None
        self._lock = threading.RLock()

    @property
    def loaded(self) -> bool:
        with self._lock:
            return self._model is not None

    def load(self) -> None:
        with self._lock:
            if self._model is not None:
                return

            from timesfm3 import TimesFM3Forecaster

            self._model = TimesFM3Forecaster.from_pretrained(
                self.config.checkpoint,
                device=self.config.device,
                per_core_batch_size=self.config.batch_size,
                revision=self.config.revision,
                local_files_only=self.config.local_files_only,
            )

    def predict(
        self,
        target: np.ndarray,
        *,
        horizon: int,
        past_only_covariates: np.ndarray | None,
        past_future_covariates: np.ndarray | None,
        return_quantiles: bool,
    ):
        with self._lock:
            if self._model is None:
                self.load()
            return self._model.predict(
                target,
                horizon=horizon,
                past_only_covariates=past_only_covariates,
                past_future_covariates=past_future_covariates,
                return_quantiles=return_quantiles,
            )
