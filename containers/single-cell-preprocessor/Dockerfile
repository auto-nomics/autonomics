FROM crpi-isjkczwpadlvr9i3.cn-hongkong.personal.cr.aliyuncs.com/autonomics/single-cell-preprocessor@sha256:d6f13cff33052e1360b0d85c43904149e4dc75e1eae3195b62f6e9a1394fe0ab

LABEL org.opencontainers.image.title="autonomics-single-cell-preprocessor" \
      org.opencontainers.image.description="Pinned scverse runtime for MatrixMarket and H5AD single-cell workflows" \
      org.opencontainers.image.version="0.2.0" \
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

USER root

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
import celltypist, igraph, leidenalg, pyarrow
assert anndata.__version__ == "0.11.4"
assert scanpy.__version__ == "1.11.3"
assert celltypist.__version__ == "1.7.1"
assert igraph.__version__ == "1.0.0"
assert leidenalg.__version__ == "0.12.0"
assert pyarrow.__version__ == "25.0.1"
PY

COPY preprocess.py /opt/autonomics/preprocess.py
COPY workflow.py /opt/autonomics/workflow.py
RUN chmod 0555 /opt/autonomics/preprocess.py /opt/autonomics/workflow.py

ENV HOME=/tmp \
    XDG_CACHE_HOME=/tmp/cache \
    MPLCONFIGDIR=/tmp/mplconfig

USER 1000:1000

WORKDIR /work

CMD ["python", "/opt/autonomics/preprocess.py"]
