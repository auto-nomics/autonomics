FROM docker.io/library/python:3.11.11-slim

LABEL org.opencontainers.image.title="autonomics-single-cell-preprocessor" \
      org.opencontainers.image.description="Pinned Scanpy runtime for MatrixMarket single-cell ingestion and inspection" \
      org.opencontainers.image.version="0.1.0" \
      org.opencontainers.image.source="https://github.com/scverse/scanpy" \
      org.opencontainers.image.licenses="BSD-3-Clause"

ENV PYTHONDONTWRITEBYTECODE=1 \
    PYTHONUNBUFFERED=1 \
    PIP_NO_CACHE_DIR=1 \
    OMP_NUM_THREADS=1 \
    OPENBLAS_NUM_THREADS=1 \
    MKL_NUM_THREADS=1 \
    NUMEXPR_MAX_THREADS=1

WORKDIR /app

COPY requirements.txt /app/requirements.txt
RUN python -m pip install --no-cache-dir -r /app/requirements.txt \
    && python - <<'PY'
import anndata, scanpy
assert anndata.__version__ == "0.11.4"
assert scanpy.__version__ == "1.11.3"
PY

COPY preprocess.py /opt/autonomics/preprocess.py
RUN chmod 0555 /opt/autonomics/preprocess.py \
    && mkdir -p /tmp \
    && chmod 1777 /tmp

WORKDIR /work

CMD ["python", "/opt/autonomics/preprocess.py"]
