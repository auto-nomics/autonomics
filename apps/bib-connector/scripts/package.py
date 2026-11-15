"""Build a deterministic unpacked/ZIP extension without runtime dependencies."""
from pathlib import Path
import hashlib
import json
import shutil
import zipfile

root = Path(__file__).resolve().parent.parent
files = ("client.js", "extract.js", "manifest.json", "popup.html", "popup.js", "style.css")
manifest = json.loads((root / "manifest.json").read_text())
assert manifest["manifest_version"] == 3
assert manifest["action"]["default_popup"] in files
assert manifest["permissions"] == ["activeTab", "scripting", "storage"]
assert "host_permissions" not in manifest
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
