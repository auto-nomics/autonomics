FROM docker.io/library/python:3.11.11-slim@sha256:a8e0a3090316aed0b11037aac613aef32fb1747dcc1dcb5c0f6c727a0113a07f

ENV PYTHONDONTWRITEBYTECODE=1 \
    PYTHONUNBUFFERED=1

WORKDIR /work

RUN apt-get update \
    && apt-get install -y --no-install-recommends gcc libc6-dev \
    && rm -rf /var/lib/apt/lists/* \
    && python -m pip install --no-cache-dir \
      numpy==1.26.4 \
      SimpleITK==2.3.1 \
      PyWavelets==1.5.0 \
      pykwalify==1.8.0 \
      six==1.16.0 \
      pyarrow==15.0.2 \
      pandas==2.2.3 \
      pydicom==2.4.4 \
      scikit-image==0.22.0 \
    && python -c "import pathlib, tarfile, urllib.request; urllib.request.urlretrieve('https://github.com/AIM-Harvard/pyradiomics/archive/refs/tags/v3.1.0.tar.gz', '/tmp/pyradiomics.tar.gz'); tarfile.open('/tmp/pyradiomics.tar.gz').extractall('/tmp'); pyproject=pathlib.Path('/tmp/pyradiomics-3.1.0/pyproject.toml'); pyproject.write_text(pyproject.read_text().replace('version = \"3.0.1a1\"', 'version = \"3.1.0\"'))" \
    && python -c "import pathlib; setup=pathlib.Path('/tmp/pyradiomics-3.1.0/setup.py'); text=setup.read_text(); text=text.replace('import versioneer\n', '').replace('commands = versioneer.get_cmdclass()', 'commands = {}').replace('version=versioneer.get_version(),', \"version='3.1.0',\"); setup.write_text(text)" \
    && python -m pip install --no-cache-dir --no-deps --no-build-isolation /tmp/pyradiomics-3.1.0 \
    && mkdir -p /opt/radiomics

RUN python -m pip install --no-cache-dir trimesh==4.6.13

COPY radiomics_runner.py /opt/radiomics/radiomics_runner.py
RUN chmod 0644 /opt/radiomics/radiomics_runner.py

ENTRYPOINT ["python", "/opt/radiomics/radiomics_runner.py"]
