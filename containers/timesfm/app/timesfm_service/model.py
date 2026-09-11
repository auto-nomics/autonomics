from __future__ import annotations

import threading

import numpy as np

from .config import ServiceConfig


class TimesFMPool:
    """Loads TimesFM 2.5 once and serializes access from worker threads."""

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

            import timesfm

            self._model = timesfm.TimesFM_2p5_200M_torch.from_pretrained(
                self.config.checkpoint,
                revision=self.config.revision,
                torch_compile=False,
                local_files_only=self.config.local_files_only,
            )
            self._model.compile(forecast_config(max_context=1024, max_horizon=256))

    def forecast(self, inputs: list[np.ndarray], horizon: int):
        with self._lock:
            if self._model is None:
                self.load()
            return self._model.forecast(horizon=horizon, inputs=inputs)

    def predict(self, target: np.ndarray, *, horizon: int, **_: object):
        inputs = target if target.ndim == 2 else target[None, :]
        return self.forecast(list(inputs), horizon=horizon)


def forecast_config(max_context: int, max_horizon: int):
    import timesfm

    return timesfm.ForecastConfig(
        max_context=max_context,
        max_horizon=max_horizon,
        normalize_inputs=True,
        use_continuous_quantile_head=True,
        force_flip_invariance=True,
        infer_is_positive=True,
        fix_quantile_crossing=True,
        per_core_batch_size=1,
    )
