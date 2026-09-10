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
                past_only_covariates=past_only,
                past_future_covariates=past_future,
                return_quantiles=request.return_quantiles,
            )
        except Exception:
            logger.exception("TimesFM forecast failed")
            raise HTTPException(status_code=502, detail="forecast failed") from None

        forecast = np.asarray(output.forecast, dtype=float)
        if forecast.ndim == 1:
            forecast = forecast[None, :]

        quantiles = None
        if request.return_quantiles and output.quantiles is not None:
            quantile_array = np.asarray(output.quantiles, dtype=float)
            if quantile_array.ndim == 2:
                quantile_array = quantile_array[None, :, :]
            quantiles = quantile_array.tolist()

        return ForecastResponse(
            ts_id=request.ts_id,
            horizon=request.horizon,
            forecast=forecast.tolist(),
            quantiles=quantiles,
            model=service_config.checkpoint,
        )

    app.state.model_pool = pool
    return app


app = create_app()
