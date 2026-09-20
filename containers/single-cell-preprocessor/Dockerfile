FROM crpi-isjkczwpadlvr9i3.cn-hongkong.personal.cr.aliyuncs.com/autonomics/single-cell-preprocessor@sha256:7a7397f45775a4c4b6c4c220711db2b95dd37fdc40b7b7f06d181a220903e467

LABEL org.opencontainers.image.title="autonomics-single-cell-preprocessor" \
      org.opencontainers.image.description="Pinned scverse runtime for MatrixMarket and H5AD single-cell workflows" \
      org.opencontainers.image.version="0.2.1" \
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

USER root

COPY preprocess.py /opt/autonomics/preprocess.py
COPY workflow.py /opt/autonomics/workflow.py
RUN chmod 0555 /opt/autonomics/preprocess.py /opt/autonomics/workflow.py

ENV HOME=/tmp \
    XDG_CACHE_HOME=/tmp/cache \
    MPLCONFIGDIR=/tmp/mplconfig

USER 1000:1000

WORKDIR /work

CMD ["python", "/opt/autonomics/preprocess.py"]
