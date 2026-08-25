FROM python:3.11-slim-bookworm AS builder

ENV PIP_NO_CACHE_DIR=1 \
  POETRY_VIRTUALENVS_CREATE=false \
  POETRY_NO_INTERACTION=1

RUN python -m pip install "poetry==1.8.4" \
  && python -m venv /opt/venv

ENV PATH="/opt/venv/bin:$PATH"

WORKDIR /src

COPY ./ldsc-python3/pyproject.toml ./ldsc-python3/poetry.lock ./ldsc-python3/README.md ./
RUN poetry check --lock \
  && poetry export --only main --format requirements.txt --without-hashes \
     --output requirements.txt \
  && /opt/venv/bin/pip install --requirement requirements.txt

FROM python:3.11-slim-bookworm

LABEL org.opencontainers.image.title="autonomics-ldsc" \
  org.opencontainers.image.version="2.0.0" \
  org.opencontainers.image.source="https://github.com/abdenlab/ldsc-python3" \
  org.opencontainers.image.revision="b6176a4"

COPY --from=builder /opt/venv /opt/venv

COPY ldsc-python3/ldsc.py ldsc-python3/munge_sumstats.py /opt/ldsc/
COPY ldsc-python3/ldscore/ /opt/ldsc/ldscore/

RUN chmod +x /opt/ldsc/ldsc.py /opt/ldsc/munge_sumstats.py \
  && ln -s /opt/ldsc/ldsc.py /opt/venv/bin/ldsc \
  && ln -s /opt/ldsc/munge_sumstats.py /opt/venv/bin/munge_sumstats

ENV PYTHONUNBUFFERED=1 \
  PYTHONPATH=/opt/ldsc \
  PATH="/opt/venv/bin:$PATH"

WORKDIR /work

ENTRYPOINT ["ldsc"]
