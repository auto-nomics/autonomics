FROM python:2.7.18-buster AS bitarray-builder

ENV SOURCE_DATE_EPOCH=1678212919

RUN PYTHONDONTWRITEBYTECODE=1 python -m pip install \
    --no-cache-dir --no-compile bitarray==0.8.1

FROM python:2.7.18-slim-buster

LABEL org.opencontainers.image.title="autonomics-mtag-original" \
  org.opencontainers.image.version="1.0.8" \
  org.opencontainers.image.source="https://github.com/JonJala/mtag.git" \
  org.opencontainers.image.revision="9e17f3cf1fbcf57b6bc466daefdc51fd0de3c5dc"

RUN python -m pip install --no-cache-dir \
    numpy==1.16.6 \
    pandas==0.24.2 \
    scipy==1.2.3 \
    joblib==0.14.1 \
    python-dateutil==2.9.0.post0 \
    pytz==2026.3.post1 \
    six==1.17.0

COPY --from=bitarray-builder \
  /usr/local/lib/python2.7/site-packages/bitarray/ \
  /usr/local/lib/python2.7/site-packages/bitarray/

WORKDIR /opt/mtag

COPY mtag/LICENSE mtag/mtag.py mtag/mtag_munge.py ./
COPY mtag/ldsc_mod/__init__.py ./ldsc_mod/
COPY mtag/ldsc_mod/ldscore/ ./ldscore_staging/
RUN mkdir ./ldsc_mod/ldscore \
  && cp ./ldscore_staging/* ./ldsc_mod/ldscore/ \
  && rm -rf ./ldscore_staging \
  && ln -s /opt/mtag/mtag.py /usr/local/bin/mtag

ENV PYTHONUNBUFFERED=1 \
  PYTHONDONTWRITEBYTECODE=1 \
  PYTHONPATH=/opt/mtag

USER 65534:65534

WORKDIR /work

ENTRYPOINT ["mtag"]
