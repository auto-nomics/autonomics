FROM docker.io/library/python:3.11.11-slim

ARG TIMESFM_REVISION=1d952420fba87f3c6dee4f240de0f1a0fbc790e3

LABEL org.opencontainers.image.title="autonomics-timesfm" \
  org.opencontainers.image.version="2.5-200m-r1" \
  org.opencontainers.image.source="https://github.com/google-research/timesfm" \
  org.opencontainers.image.licenses="Apache-2.0" \
  io.autonomics.model.checkpoint="google/timesfm-2.5-200m-pytorch@${TIMESFM_REVISION}"

ENV PYTHONDONTWRITEBYTECODE=1 \
  PYTHONUNBUFFERED=1 \
  PIP_NO_CACHE_DIR=1 \
  TIMESFM_CHECKPOINT=/opt/timesfm/checkpoint \
  TIMESFM_REVISION=${TIMESFM_REVISION}

WORKDIR /app

COPY requirements.txt requirements.txt
RUN python -m pip install --no-cache-dir -r requirements.txt

# TimesFM 3 weights cannot be redistributed. TimesFM 2.5 weights are
# Apache-2.0 and make this image usable with the DAG runtime's isolated network.
RUN hf download google/timesfm-2.5-200m-pytorch \
    --revision "$TIMESFM_REVISION" \
    --include config.json \
    --include model.safetensors \
    --local-dir /opt/timesfm/checkpoint \
    --format quiet \
  && test -s /opt/timesfm/checkpoint/config.json \
  && test -s /opt/timesfm/checkpoint/model.safetensors \
  && echo "2f776efe6245e42b24bc4153ffdf61810140210e4bd3b01fb21f7aa779ab6ce8  /opt/timesfm/checkpoint/model.safetensors" \
    | sha256sum -c -

COPY app /app

RUN groupadd --gid 1000 timesfm \
  && useradd --uid 1000 --gid timesfm --create-home timesfm \
  && mkdir -p /cache/timesfm \
  && chown -R timesfm:timesfm /app /cache/timesfm

USER timesfm:timesfm
EXPOSE 8000

HEALTHCHECK --interval=30s --timeout=5s --start-period=10s --retries=3 \
  CMD python -c "import urllib.request; urllib.request.urlopen('http://127.0.0.1:8000/healthz', timeout=3)"

CMD ["uvicorn", "timesfm_service.main:app", "--host", "0.0.0.0", "--port", "8000"]
