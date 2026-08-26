FROM docker.io/library/python:3.10-slim-bookworm AS build

RUN apt-get update && \
    apt-get install -y --no-install-recommends \
      build-essential \
      cmake \
      libboost-date-time-dev \
      libboost-filesystem-dev \
      libboost-program-options-dev \
      libboost-system-dev && \
    rm -rf /var/lib/apt/lists/*

WORKDIR /build
COPY src/ /build/src/
WORKDIR /build/src
RUN cmake -S . -B build \
      -DCMAKE_BUILD_TYPE=Release \
      -DBoost_NO_BOOST_CMAKE=ON && \
    cmake --build build --target bgmg --parallel 2

FROM docker.io/library/python:3.10-slim-bookworm

LABEL org.opencontainers.image.title="autonomics-mixer-original" \
  org.opencontainers.image.version="2.2.1" \
  org.opencontainers.image.source="https://github.com/precimed/gsa-mixer" \
  org.opencontainers.image.revision="ea2a445912f83e5767d67372b6075912ed5655d8" \
  org.opencontainers.image.licenses="GPL-3.0-only"

RUN apt-get update && \
    apt-get install -y --no-install-recommends \
      libboost-date-time1.74.0 \
      libboost-filesystem1.74.0 \
      libboost-program-options1.74.0 \
      libboost-system1.74.0 \
      libgomp1 && \
    rm -rf /var/lib/apt/lists/*

RUN pip install --no-cache-dir \
      intervaltree==3.1.0 \
      numdifftools==0.9.39 \
      numpy==1.23.3 \
      packaging==26.3 \
      pandas==1.5.0 \
      python-dateutil==2.9.0.post0 \
      pytz==2026.3.post1 \
      scipy==1.9.1 \
      six==1.16.0 \
      sortedcontainers==2.4.0

ENV PYTHONDONTWRITEBYTECODE=1 \
    PYTHONUNBUFFERED=1 \
    BGMG_SHARED_LIBRARY=/tools/mixer/lib/libbgmg.so

WORKDIR /tools/mixer
COPY --from=build /build/src/build/lib/libbgmg.so /tools/mixer/lib/libbgmg.so
COPY precimed/mixer.py /tools/mixer/precimed/mixer.py
COPY precimed/version.py /tools/mixer/precimed/version.py
COPY precimed/common/ /tools/mixer/precimed/common/
COPY precimed/bivar_mixer/ /tools/mixer/precimed/bivar_mixer/
COPY precimed/gsa_mixer/ /tools/mixer/precimed/gsa_mixer/

WORKDIR /work
ENTRYPOINT ["python", "/tools/mixer/precimed/mixer.py"]
