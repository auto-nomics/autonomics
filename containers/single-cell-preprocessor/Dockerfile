FROM docker.io/library/python:3.11.11-slim@sha256:a8e0a3090316aed0b11037aac613aef32fb1747dcc1dcb5c0f6c727a0113a07f

LABEL org.opencontainers.image.title="autonomics-single-cell-preprocessor" \
      org.opencontainers.image.description="Pinned Scanpy runtime for MatrixMarket single-cell ingestion and inspection" \
      org.opencontainers.image.version="0.1.0" \
      org.opencontainers.image.source="https://github.com/scverse/scanpy" \
      org.opencontainers.image.licenses="BSD-3-Clause"

ENV PYTHONDONTWRITEBYTECODE=1 \
    PYTHONUNBUFFERED=1 \
    PIP_NO_CACHE_DIR=1 \
    PIP_DISABLE_PIP_VERSION_CHECK=1 \
    OMP_NUM_THREADS=1 \
    OPENBLAS_NUM_THREADS=1 \
    MKL_NUM_THREADS=1 \
    NUMEXPR_MAX_THREADS=1

WORKDIR /app

COPY requirements.txt /app/requirements.txt
RUN python -m pip install \
      --no-cache-dir \
      --only-binary=:all: \
      --no-deps \
      --no-compile \
      -r /app/requirements.txt \
    && python -m pip check \
    && python - <<'PY'
import anndata, scanpy
assert anndata.__version__ == "0.11.4"
assert scanpy.__version__ == "1.11.3"
PY

COPY preprocess.py /opt/autonomics/preprocess.py
RUN chmod 0555 /opt/autonomics/preprocess.py

RUN groupadd --gid 1000 autonomics \
    && useradd --uid 1000 --gid autonomics --no-create-home autonomics

ENV HOME=/tmp \
    XDG_CACHE_HOME=/tmp/cache \
    MPLCONFIGDIR=/tmp/mplconfig

USER 1000:1000

WORKDIR /work

CMD ["python", "/opt/autonomics/preprocess.py"]
