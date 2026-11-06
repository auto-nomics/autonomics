#!/usr/bin/env python3
"""Build the pinned VS Code OSS web server and its container native modules."""

import argparse
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile


COMMIT = "2bde2b461e21697033eb597d74cb7719caeb0332"
MODULES = {
    "@vscode/spdlog": "spdlog.node",
    "@parcel/watcher": "watcher.node",
    "@vscode/sqlite3": "vscode-sqlite3.node",
    "node-pty": "pty.node",
    "kerberos": "kerberos.node",
}


def run(args, **kwargs):
    print("+", " ".join(map(str, args)), flush=True)
    subprocess.run(args, check=True, **kwargs)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--source", type=Path, default=Path("reference/vscode"))
    parser.add_argument("--image", required=True, help="Local build image with g++, make, Python and Kerberos headers")
    parser.add_argument("--storage-root", type=Path, help="Podman storage root (use with --run-root)")
    parser.add_argument("--run-root", type=Path, help="Podman run root (use with --storage-root)")
    parser.add_argument("--storage-driver", default="overlay")
    parser.add_argument("--reuse-bundle", action="store_true", help="Reuse an existing out-vscode-reh-web bundle")
    args = parser.parse_args()
    if bool(args.storage_root) != bool(args.run_root):
        parser.error("--storage-root and --run-root must be supplied together")

    source = args.source.resolve(strict=True)
    commit = subprocess.check_output(["git", "-C", str(source), "rev-parse", "HEAD"], text=True).strip()
    if commit != COMMIT:
        raise RuntimeError(f"Expected VS Code source {COMMIT}, found {commit}")
    output = source.parent / "vscode-reh-web-linux-x64"
    version = (source / ".nvmrc").read_text().strip()
    headers = Path.home() / ".cache" / "node-gyp" / version
    toolchain = source / ".build" / "toolchain" / "node_modules" / ".bin"
    node, pnpm = toolchain / "node", toolchain / "pnpm"
    node_gyp = source / ".build" / "toolchain" / "node_modules" / "pnpm" / "dist" / "node_modules" / "node-gyp" / "bin" / "node-gyp.js"
    for path in (node, pnpm, node_gyp, headers, source / "remote" / "node_modules"):
        if not path.exists():
            raise RuntimeError(f"Missing {path}; install the pinned VS Code build dependencies first")
    env = os.environ.copy()
    env["PATH"] = f"{toolchain}{os.pathsep}{env.get('PATH', '')}"
    gulp = [str(pnpm), "run", "gulp"]

    if not args.reuse_bundle:
        # This pinned upstream revision strips the Copilot SDK before its final
        # packaging step. Only recover from that specific, known build failure.
        with tempfile.TemporaryFile(mode="w+t") as log:
            result = subprocess.run(gulp + ["vscode-reh-web-linux-x64"], cwd=source, env=env,
                                    stdout=log, stderr=subprocess.STDOUT)
            log.seek(0)
            output_log = log.read()
        print(output_log, flush=True)
        if result.returncode and "Copilot SDK directory not found" not in output_log:
            raise RuntimeError("VS Code build failed; see the output above")
        if result.returncode == 0:
            print("Upstream packaging succeeded without the SDK workaround.", flush=True)
    elif not (source / "out-vscode-reh-web" / "server-main.js").is_file():
        raise RuntimeError("--reuse-bundle requires an existing out-vscode-reh-web bundle")

    sdk = source / "extensions" / "copilot" / "node_modules" / "@github" / "copilot"
    staged_sdk = source / ".build" / "extensions" / "copilot" / "node_modules" / "@github" / "copilot"
    if not (sdk / "sdk").is_dir() or not staged_sdk.is_dir():
        raise RuntimeError("Built-in Copilot SDK or extension staging directory is missing")
    shutil.copytree(sdk, staged_sdk, dirs_exist_ok=True, symlinks=True)
    run(gulp + ["vscode-reh-web-linux-x64-ci"], cwd=source, env=env)

    podman = ["podman"]
    if args.storage_root:
        run_root = args.run_root.resolve()
        podman += ["--root", str(args.storage_root.resolve()), "--runroot", str(run_root),
                   "--tmpdir", str(run_root / "libpod-tmp"), "--storage-driver", args.storage_driver]
    for path in (source, output, headers):
        if ":" in str(path):
            raise RuntimeError(f"Container mount path contains ':': {path}")
    command = ('set -eu; export PATH=/server:$PATH; '
               'for package in @vscode/spdlog @parcel/watcher @vscode/sqlite3 node-pty kerberos; do '
               'cd /src/remote/node_modules/$package; '
               'node /src/.build/toolchain/node_modules/pnpm/dist/node_modules/node-gyp/bin/node-gyp.js '
               'rebuild --nodedir=/headers; done')
    run(podman + ["run", "--rm", "--network", "none", "--pull=never", "-v", f"{source}:/src:rw",
                  "-v", f"{output}:/server:ro", "-v", f"{headers}:/headers:ro", "--entrypoint", "/bin/sh",
                  args.image, "-c", command])
    for package, filename in MODULES.items():
        relative = Path(package) / "build" / "Release" / filename
        shutil.copy2(source / "remote" / "node_modules" / relative, output / "node_modules" / relative)

    bridge = Path(__file__).resolve().parent.parent / "apps" / "vscode-bridge"
    shutil.copytree(bridge, output / "extensions" / "autonomics-workspace-bridge", dirs_exist_ok=True)

    check = "for (const p of " + json.dumps(list(MODULES)) + ") { require('/server/node_modules/'+p); console.log('loaded', p); }"
    run(podman + ["run", "--rm", "--network", "none", "--pull=never", "-v", f"{output}:/server:ro",
                  "--entrypoint", "/server/node", args.image, "-e", check])
    print(f"VS Code OSS server: {output}")


if __name__ == "__main__":
    try:
        main()
    except (RuntimeError, subprocess.CalledProcessError) as error:
        print(f"Build failed: {error}", file=sys.stderr)
        sys.exit(1)
