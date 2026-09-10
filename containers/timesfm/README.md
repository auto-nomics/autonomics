# TimesFM service container

This directory contains a reusable HTTP inference-node skeleton for
[TimesFM](https://github.com/google-research/timesfm) 3.0. The image contains
only code and runtime dependencies; the pretrained checkpoint is downloaded on
first model load and cached under `/cache/timesfm`.

## API

```sh
curl http://localhost:8000/healthz
curl http://localhost:8000/readyz
```

Univariate forecast:

```sh
curl -X POST http://localhost:8000/v1/forecasts \
  -H 'content-type: application/json' \
  -d '{"target":[1,2,3,4,5,6],"horizon":3,"return_quantiles":true}'
```

For multivariate forecasting, `target` is a list of rows with shape
`(num_variates, context_length)`. `past_only_covariates` has shape
`(num_covariates, context_length)`. `past_future_covariates` has shape
`(num_covariates, context_length + horizon)`.

## Build and run

```sh
podman build \
  -f containers/timesfm/Dockerfile \
  -t localhost/atc/timesfm:3.0.2 \
  containers/timesfm

podman run --rm -p 8000:8000 \
  -v timesfm-cache:/cache/timesfm \
  -e TIMESFM_DEVICE=cpu \
  localhost/atc/timesfm:3.0.2
```

The first `/v1/forecasts` request downloads the checkpoint and may take several
minutes. For a long-running service, set `TIMESFM_EAGER_LOAD=true` so startup
loads the model once and `/readyz` remains non-ready until loading completes.

GPU deployments need a CUDA-capable PyTorch base or wheel set and the NVIDIA
container runtime. The current build uses CPU wheels so the image remains a
portable development skeleton.

## Tests

Run the local API contract tests:

```sh
python -m pip install -r containers/timesfm/requirements-dev.txt
python -m pytest containers/timesfm/app/tests
```

Build the image and test health without downloading weights:

```sh
containers/timesfm/test_timesfm_service.sh
```

## Checkpoint and license

The default `google/timesfm-3.0-pytorch` weights are distributed under
`timesfm-non-commercial-license-v1.0`, not Apache-2.0. Review that license
before using the default checkpoint in production or commercial workflows.
Set `TIMESFM_CHECKPOINT` and `TIMESFM_REVISION` to select another compatible
checkpoint.
