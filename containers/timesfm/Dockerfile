FROM docker.io/library/python:3.11.11-slim@sha256:a8e0a3090316aed0b11067aac613aef32fb1747dcc1dcb5c0f6c727a0113a07f

LABEL org.opencontainers.image.title="autonomics-timesfm" \
  org.opencontainers.image.version="3.0.2" \
  org.opencontainers.image.source="https://github.com/google-research/timesfm" \
  org.opencontainers.image.licenses="Apache-2.0"

ENV PYTHONDONTWRITEBYTECODE=1 \
  PYTHONUNBUFFERED=1 \
  PIP_NO_CACHE_DIR=1 \
  HF_HOME=/cache/timesfm/huggingface

WORKDIR /app

COPY requirements.txt requirements.txt
RUN python -m pip install --no-cache-dir -r requirements.txt

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
