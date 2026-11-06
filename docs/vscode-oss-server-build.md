# VS Code OSS server build

This procedure builds an experimental Linux x64 server for the plugin
editor. It does not change a running gateway or install extensions into a
user's profile.

The source checkout must be at `reference/vscode`, at commit
`2bde2b461e21697033eb597d74cb7719caeb0332`. Install the Node and pnpm versions
declared by that checkout into its ignored build directory, then install the
upstream dependencies:

```sh
cd reference/vscode
npm install --prefix .build/toolchain --no-audit --no-fund node@24.22.1 pnpm@12.16.0
PATH="$PWD/.build/toolchain/node_modules/.bin:$PATH" pnpm install --frozen-lockfile
cd ../..
```

Run the build using a local Debian-compatible image that has a C/C++ compiler,
make, Python and Kerberos development headers. For the configured private
Podman overlay store, for example:

```sh
python scripts/build-vscode-oss-server.py \
  --image YOUR_LOCAL_BUILD_IMAGE \
  --storage-root /mnt/data/containers-overlay/storage \
  --run-root /mnt/data/containers-overlay/run
```

Omit both storage options to use Podman's normal configuration. The build uses
the supplied image to compile the native modules with that image's toolchain;
the resulting binaries must still be checked against each target image's libc,
C++ runtime and architecture. It outputs to the ignored directory
`reference/vscode-reh-web-linux-x64` and recreates that directory on each run.
The script also tests that the rebuilt modules load inside the build image.
Use `--reuse-bundle` only after the upstream web-server bundle and extension
staging have already been generated successfully.

The pinned upstream build currently removes the built-in Copilot SDK before
its final packaging step. The script detects that specific failure, restores
the matching SDK to the generated staging directory and repeats packaging.
Other build failures stop the script.

For gateway integration development, setting `AUTONOMICS_VSCODE_SERVER_DIR` to
the absolute output directory makes a newly opened project start the server in
that project's dev container. The gateway creates a private host directory
for its Unix socket and closes the server with the project session. A missing
or incompatible server prevents that project from opening. This setting is
experimental: the gateway issues a per-project capability URL on a separate
`127.0.0.2` loopback origin. It is available only when the gateway is accessed
through `127.0.0.1` or `localhost`; it does not support a remote browser. The
embedded workbench and the existing project editor are separate views, and
their sidebar and terminal integration is not yet complete.

The server's extension and remote state directory is persisted per project at
`<configured container workspace>/.devcontainer-vscode/<project>`. Its parent
is private to the gateway user; each project directory is mounted only into
that project's container. This directory is outside the plugin project tree.
The gateway records a separate loopback port for each project alongside these
directories, so the browser origin remains stable after a container restart.
The access capability in the URL changes on each connection. If the recorded
port is occupied, opening the workbench fails instead of silently switching
origins; stop the conflicting listener before retrying. Browser-local state
still depends on the browser profile and its storage settings.
