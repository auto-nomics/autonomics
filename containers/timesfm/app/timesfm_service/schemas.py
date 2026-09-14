from __future__ import annotations

from typing import Annotated, Literal

import numpy as np
from pydantic import BaseModel, Field


Array1D = Annotated[list[float], Field(min_length=1)]
Array2D = Annotated[list[Array1D], Field(min_length=1)]


class ForecastRequest(BaseModel):
    target: Array1D | Array2D
    horizon: Annotated[int, Field(ge=1, le=1000)] = 24
    past_only_covariates: Array2D | None = None
    past_future_covariates: Array2D | None = None
    return_quantiles: bool = True
    ts_id: str | None = Field(default=None, max_length=256)


class ForecastResponse(BaseModel):
    ts_id: str | None
    horizon: int
    forecast: list[list[float]]
    quantiles: list[list[list[float]]] | None
    model: str
    checkpoint_revision: str


class HealthResponse(BaseModel):
    status: Literal["ok"]
    model_loaded: bool
    checkpoint: str


class ReadyResponse(BaseModel):
    status: Literal["ready", "loading"]
    model_loaded: bool


def as_array(value: list[float] | list[list[float]]) -> np.ndarray:
    array = np.asarray(value, dtype=np.float32)
    if array.ndim not in {1, 2}:
        raise ValueError("target must be one- or two-dimensional")
    if not np.isfinite(array).all():
        raise ValueError("target and covariates must contain finite values")
    return array


ForecastRequest.model_rebuild()
