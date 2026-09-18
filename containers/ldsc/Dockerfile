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

# LDSC 3.0.1 filters incompatible allele pairs but passes the pre-filter
# allele Series to alignment, so any rejected row still raises KeyError.
# Keep the official source as the build context and apply this minimal fix
# deterministically; the assertion prevents a silent upstream code drift.
RUN python - <<'PY'
from pathlib import Path

path = Path("/opt/ldsc/ldscore/sumstats.py")
text = path.read_text()
old = """\
        loop = _select_and_log(loop, _filter_alleles(alleles), log,
                               '{N} SNPs with valid alleles.')
        loop['Z2'] = _align_alleles(loop.Z2, alleles)
"""
new = """\
        valid_indices = _filter_alleles(alleles)
        loop = _select_and_log(loop, valid_indices, log,
                               '{N} SNPs with valid alleles.')
        loop['Z2'] = _align_alleles(loop.Z2, alleles[valid_indices])
"""
if old not in text:
    raise SystemExit("expected LDSC 3.0.1 allele-filter block was not found")
path.write_text(text.replace(old, new, 1))
PY

RUN chmod +x /opt/ldsc/ldsc.py /opt/ldsc/munge_sumstats.py \
  && ln -s /opt/ldsc/ldsc.py /usr/local/bin/ldsc \
  && ln -s /opt/ldsc/munge_sumstats.py /usr/local/bin/munge_sumstats

ENV PYTHONUNBUFFERED=1 \
  PYTHONDONTWRITEBYTECODE=1 \
  PYTHONPATH=/opt/ldsc

WORKDIR /work

ENTRYPOINT ["ldsc"]
