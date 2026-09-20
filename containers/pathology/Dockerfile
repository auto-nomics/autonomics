# CPU variant — built and smoke-tested locally (no NVIDIA GPU on this host).
# Production GPU hosts use Dockerfile.cuda; both share pathology_runner.py.
FROM docker.io/library/python:3.11.11-slim@sha256:a8e0a3090316aed0b11037aac613aef32fb1747dcc1dcb5c0f6c727a0113a07f

ENV PYTHONDONTWRITEBYTECODE=1 \
    PYTHONUNBUFFERED=1

WORKDIR /work

RUN apt-get update \
    && apt-get install -y --no-install-recommends libglib2.0-0 \
    && rm -rf /var/lib/apt/lists/* \
    && python -m pip install --no-cache-dir \
      --index-url https://download.pytorch.org/whl/cpu \
      --extra-index-url https://pypi.org/simple \
      torch==2.3.1 \
      torchvision==0.18.1 \
    && python -m pip install --no-cache-dir \
      numpy==1.26.4 \
      pandas==2.2.3 \
      pyarrow==15.0.2 \
      scikit-image==0.22.0 \
      scikit-learn==1.4.2 \
      scipy==1.11.4 \
      tiffslide==2.4.0 \
      zarr==2.17.2 \
      numcodecs==0.12.1 \
      tifffile==2024.2.12 \
      h5py==3.10.0 \
      timm==1.0.3 \
      pillow==10.2.0 \
    && mkdir -p /opt/pathology

COPY pathology_runner.py /opt/pathology/pathology_runner.py
RUN chmod 0644 /opt/pathology/pathology_runner.py

ENTRYPOINT ["python", "/opt/pathology/pathology_runner.py"]
