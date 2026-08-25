FROM python:3.9-slim-bookworm

LABEL org.opencontainers.image.title="autonomics-ldsc-original" \
  org.opencontainers.image.version="3.0.1" \
  org.opencontainers.image.source="file:///mnt/disk3/ldsc3/ldsc"

RUN python -m pip install --no-cache-dir \
    numpy==1.23.3 \
    pandas==1.5.0 \
    scipy==1.9.2 \
    bitarray==2.6.0

WORKDIR /opt/ldsc

COPY ldsc.py munge_sumstats.py ./
COPY ldscore/ ./ldscore/

RUN chmod +x /opt/ldsc/ldsc.py /opt/ldsc/munge_sumstats.py \
  && ln -s /opt/ldsc/ldsc.py /usr/local/bin/ldsc \
  && ln -s /opt/ldsc/munge_sumstats.py /usr/local/bin/munge_sumstats

ENV PYTHONUNBUFFERED=1 \
  PYTHONDONTWRITEBYTECODE=1 \
  PYTHONPATH=/opt/ldsc

WORKDIR /work

ENTRYPOINT ["ldsc"]
