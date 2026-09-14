from __future__ import annotations

from fastapi.testclient import TestClient

from timesfm_service.config import ServiceConfig
from timesfm_service.main import create_app


class FakeModel:
    def forecast(self, horizon, inputs):
        assert horizon == 2
        assert len(inputs) == 1
        return ([[1.0, 2.0]], [[[0.5, 0.6], [1.5, 1.6]]])


def test_health_and_forecast(monkeypatch):
    config = ServiceConfig(checkpoint="fixture-checkpoint")
    app = create_app(config)
    client = TestClient(app)

    response = client.get("/healthz")
    assert response.status_code == 200
    assert response.json()["model_loaded"] is False

    monkeypatch.setattr(app.state.model_pool, "_model", FakeModel())
    response = client.post(
        "/v1/forecasts",
        json={"target": [1.0, 2.0, 3.0], "horizon": 2},
    )
    assert response.status_code == 200
    body = response.json()
    assert body["forecast"] == [[1.0, 2.0]]
    assert body["quantiles"] == [[[0.5, 0.6], [1.5, 1.6]]]
    assert body["model"] == "google/timesfm-2.5-200m-pytorch"


def test_past_future_covariate_length_is_rejected():
    client = TestClient(create_app(ServiceConfig()))
    response = client.post(
        "/v1/forecasts",
        json={
            "target": [1.0, 2.0],
            "horizon": 2,
            "past_future_covariates": [[1.0, 2.0]],
        },
    )
    assert response.status_code == 422


def test_eager_load_reports_not_ready_before_model_load():
    client = TestClient(create_app(ServiceConfig(eager_load=True)))

    response = client.get("/readyz")

    assert response.status_code == 503
    assert response.json() == {"status": "loading", "model_loaded": False}
