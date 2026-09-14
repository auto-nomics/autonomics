from __future__ import annotations

import logging
from contextlib import asynccontextmanager

import numpy as np
from fastapi import FastAPI, HTTPException, Response
from fastapi.concurrency import run_in_threadpool

from .config import ServiceConfig, load_config
from .model import TimesFMPool
from .schemas import ForecastRequest, ForecastResponse, HealthResponse, ReadyResponse, as_array

logger = logging.getLogger(__name__)


def create_app(config: ServiceConfig | None = None) -> FastAPI:
    service_config = config or load_config()
    pool = TimesFMPool(service_config)

    @asynccontextmanager
    async def lifespan(app: FastAPI):
        if service_config.eager_load:
            await run_in_threadpool(pool.load)
        yield

    app = FastAPI(
        title="Autonomics TimesFM inference service",
        version="0.1.0",
        lifespan=lifespan,
    )

    @app.get("/healthz", response_model=HealthResponse)
    async def healthz() -> HealthResponse:
        return HealthResponse(
            status="ok",
            model_loaded=pool.loaded,
            checkpoint=service_config.checkpoint,
        )

    @app.get("/readyz", response_model=ReadyResponse)
    async def readyz(response: Response) -> ReadyResponse:
        loaded = pool.loaded
        if not loaded and service_config.eager_load:
            response.status_code = 503
        return ReadyResponse(
            status="ready" if loaded or not service_config.eager_load else "loading",
            model_loaded=loaded,
        )

    @app.post(
        "/v1/forecasts",
        response_model=ForecastResponse,
        responses={502: {"description": "Model inference failed"}},
    )
    async def forecast(request: ForecastRequest) -> ForecastResponse:
        try:
            target = as_array(request.target)
            past_only = (
                as_array(request.past_only_covariates)
                if request.past_only_covariates is not None
                else None
            )
            past_future = (
                as_array(request.past_future_covariates)
                if request.past_future_covariates is not None
                else None
            )
        except ValueError as exc:
            raise HTTPException(status_code=422, detail=str(exc)) from exc

        if request.past_only_covariates is not None or request.past_future_covariates is not None:
            raise HTTPException(
                status_code=422,
                detail=(
                    "TimesFM 2.5 service does not support covariates; "
                    "use the TimesFM 3 service with an externally mounted checkpoint"
                ),
            )

        if past_only is not None and past_only.shape[-1] != target.shape[-1]:
            raise HTTPException(
                status_code=422,
                detail="past_only_covariates context length must match target",
            )
        if (
            past_future is not None
            and past_future.shape[-1] != target.shape[-1] + request.horizon
        ):
            raise HTTPException(
                status_code=422,
                detail=(
                    "past_future_covariates length must equal target context "
                    "plus horizon"
                ),
            )

        try:
            output = await run_in_threadpool(
                pool.predict,
                target,
                horizon=request.horizon,
                return_quantiles=request.return_quantiles,
            )
        except Exception:
            logger.exception("TimesFM forecast failed")
            raise HTTPException(status_code=502, detail="forecast failed") from None

        forecast = np.asarray(output[0], dtype=float)
        if forecast.ndim == 1:
            forecast = forecast[None, :]

        quantiles = None
        if request.return_quantiles:
            quantiles = np.asarray(output[1], dtype=float).tolist()

        return ForecastResponse(
            ts_id=request.ts_id,
            horizon=request.horizon,
            forecast=forecast.tolist(),
            quantiles=quantiles,
            model="google/timesfm-2.5-200m-pytorch",
            checkpoint_revision=service_config.revision,
        )

    app.state.model_pool = pool
    return app


app = create_app()
