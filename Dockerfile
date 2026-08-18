FROM rust:1.95-bookworm AS builder

RUN apt-get update \
  && apt-get install -y --no-install-recommends \
  g++ \
        pkg-config \
        ca-certificates \
   && rm -rf /var/lib/apt/lists/*

WORKDIR /src

COPY Cargo.toml Cargo.lock ./
COPY crates ./crates
COPY apps ./apps
COPY bio_crates ./bio_crates
COPY stat_crates ./stat_crates

RUN --mount=type=cache,target=/src/target \
    --mount=type=cache,target=/usr/local/cargo/registry \
    cargo build --release --locked -p tui

# cache mount 不会留在 layer 里，需要显式拷到普通目录。
RUN --mount=type=cache,target=/src/target \
    cp target/release/tui /usr/local/bin/autonomics-tui

########## runtime ##########
FROM archlinux:base AS runtime

RUN pacman -Sy --noconfirm --needed \
      ca-certificates \
      tzdata \
      gcc-libs \
      libgomp \
      boost-libs \
      python \
      python-numpy \
      python-scipy \
      python-pandas \
  && pacman -Scc --noconfirm \
  && groupadd --gid 1000 autonomics \
  && useradd --uid 1000 --gid autonomics --create-home autonomics

ENV HOME=/data/home \
    AUTONOMICS_DATA_DIR=/data/files \
    AUTONOMICS_STATE_DIR=/data/state \
    MIXER_RESOURCE_ROOT=/mnt/data/mixer/resources \
    MIXER_PYTHON=/usr/bin/python3 \
    TZ=Asia/Shanghai

COPY --from=builder /usr/local/bin/autonomics-tui /usr/local/bin/autonomics-tui

RUN mkdir -p /data/home /data/files /data/state /app/logs \
  && chown -R autonomics:autonomics /data /app

WORKDIR /app
USER autonomics:autonomics

ENTRYPOINT ["/usr/local/bin/autonomics-tui"]
