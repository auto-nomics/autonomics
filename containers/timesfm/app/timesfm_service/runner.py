from __future__ import annotations

import json
import math
import os
from pathlib import Path

import numpy as np

from .config import load_config
from .model import forecast_config


MAX_HORIZON = 256
MAX_SERIES = 32
MAX_CONTEXT = 16384


def _series(value: object) -> list[list[float]]:
    if isinstance(value, list) and value and isinstance(value[0], list):
        if len(value) > MAX_SERIES:
            raise ValueError(f"at most {MAX_SERIES} series are supported")
        return [list(series) for series in value]
    return [list(value)]  # type: ignore[arg-type]


def _validate(series: list[list[float]], horizon: int, max_context: int) -> None:
    if not 1 <= horizon <= MAX_HORIZON:
        raise ValueError(f"horizon must be between 1 and {MAX_HORIZON}")
    if not 1 <= max_context <= MAX_CONTEXT:
        raise ValueError(f"max_context must be between 1 and {MAX_CONTEXT}")
    for index, values in enumerate(series):
        if not values:
            raise ValueError(f"series {index} cannot be empty")
        if len(values) > MAX_CONTEXT:
            raise ValueError(
                f"series {index} exceeds the {MAX_CONTEXT}-point context limit"
            )
        if any(not math.isfinite(value) for value in values):
            raise ValueError(f"series {index} must contain only finite values")


def run() -> None:
    input_path = Path(os.environ["AUTONOMICS_INPUT0"])
    output_path = Path(os.environ["AUTONOMICS_OUTPUT0"])
    log_path = Path(os.environ["AUTONOMICS_OUTPUT1"])
    request = json.loads(input_path.read_text(encoding="utf-8"))
    horizon = int(os.environ.get("TIMESFM_HORIZON", request.get("horizon", 12)))
    max_context = int(os.environ.get("TIMESFM_MAX_CONTEXT", 1024))
    series = _series(request.get("target", request.get("series")))
    _validate(series, horizon, max_context)

    import timesfm

    config = load_config()
    model = timesfm.TimesFM_2p5_200M_torch.from_pretrained(
        config.checkpoint,
        revision=config.revision,
        torch_compile=False,
        local_files_only=True,
    )
    model.compile(forecast_config(max_context, 256))
    point_forecast, quantile_forecast = model.forecast(
        horizon=horizon,
        inputs=[np.asarray(values, dtype=np.float32) for values in series],
    )

    result = {
        "schema_version": 1,
        "model": "google/timesfm-2.5-200m-pytorch",
        "checkpoint_revision": config.revision,
        "horizon": horizon,
        "max_context": max_context,
        "forecast": np.asarray(point_forecast, dtype=float).tolist(),
        "quantile_levels": ["mean", 0.1, 0.2, 0.3, 0.4, 0.5, 0.6, 0.7, 0.8, 0.9],
        "quantiles": np.asarray(quantile_forecast, dtype=float).tolist(),
    }
    output_path.write_text(
        json.dumps(result, indent=2, sort_keys=True, allow_nan=False) + "\n",
        encoding="utf-8",
    )
    log_path.write_text(
        "TimesFM 2.5 forecast completed\n"
        f"checkpoint_revision={config.revision}\n"
        f"series={len(series)}\n"
        f"horizon={horizon}\n",
        encoding="utf-8",
    )


if __name__ == "__main__":
    run()
