"""Build a deterministic unpacked/ZIP extension without runtime dependencies."""
from pathlib import Path
import hashlib
import json
import shutil
import zipfile

root = Path(__file__).resolve().parent.parent
files = ("background.js", "client.js", "content.js", "manifest.json", "popup.html", "popup.js",
         "progress.html", "progress.js", "style.css", "translators.js")
manifest = json.loads((root / "manifest.json").read_text())
assert manifest["manifest_version"] == 3
referenced = [manifest["background"]["service_worker"], manifest["options_ui"]["page"].split("?")[0],
              *[script for cs in manifest["content_scripts"] for script in cs["js"]],
              *manifest["web_accessible_resources"][0]["resources"]]
assert all(name in files for name in referenced), referenced
assert manifest["permissions"] == ["activeTab", "scripting", "storage", "contextMenus"]
assert "host_permissions" not in manifest
assert "default_popup" not in manifest["action"]
output = root / "dist"
unpacked = output / "unpacked"
unpacked.mkdir(parents=True, exist_ok=True)
for path in unpacked.iterdir():
    if path.is_file() and path.name not in files:
        path.unlink()
archive = output / "autonomics-bib-connector.zip"
with zipfile.ZipFile(archive, "w", compression=zipfile.ZIP_STORED) as bundle:
    for name in files:
        source = root / name
        shutil.copyfile(source, unpacked / name)
        info = zipfile.ZipInfo(name, date_time=(1980, 1, 1, 0, 0, 0))
        info.create_system = 3
        info.external_attr = 0o100644 << 16
        bundle.writestr(info, source.read_bytes())
checksum = hashlib.sha256(archive.read_bytes()).hexdigest()
(output / "SHA256SUMS").write_text(f"{checksum}  {archive.name}\n")
with zipfile.ZipFile(archive) as bundle:
    assert sorted(bundle.namelist()) == sorted(files)
    assert bundle.testzip() is None
print(f"Built {archive.relative_to(root)} and dist/unpacked ({checksum})")
