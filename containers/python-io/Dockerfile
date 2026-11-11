FROM docker.io/library/python:3.12-slim-bookworm@sha256:935c3c5b12357a7e10caa9c1e6ca5692344d97cf229832ef584be2cec9fc03df

ENV PYTHONDONTWRITEBYTECODE=1 \
    PYTHONUNBUFFERED=1 \
    PIP_DISABLE_PIP_VERSION_CHECK=1 \
    HOME=/tmp

COPY requirements.txt /opt/python-io/requirements.txt
RUN python -m pip install --no-cache-dir --only-binary=:all: --require-hashes \
        -r /opt/python-io/requirements.txt \
    && python -m pip check

COPY smoke.py /opt/python-io/smoke.py
RUN python /opt/python-io/smoke.py

LABEL org.opencontainers.image.title="Autonomics Python IO" \
      org.opencontainers.image.description="Python runtime for tabular, file, object-store and database IO" \
      org.opencontainers.image.version="0.1.0"

WORKDIR /work
USER 65532:65532
CMD ["python"]
