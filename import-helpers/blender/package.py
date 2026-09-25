# SPDX-License-Identifier: MIT OR Apache-2.0
# Copyright (c) 2026 EllisonDigital
"""Build an installable legacy add-on ZIP, without importing bpy."""
from pathlib import Path
from zipfile import ZipFile, ZIP_DEFLATED
root = Path(__file__).resolve().parent
out = root.parent / ".work" / "openterrainstudio-blender.zip"
out.parent.mkdir(exist_ok=True)
with ZipFile(out, "w", ZIP_DEFLATED) as archive:
    for path in sorted((root / "openterrainstudio_import").glob("*.py")):
        archive.write(path, "openterrainstudio_import/" + path.name)
    for name in ("LICENSE-MIT", "LICENSE-APACHE"):
        archive.write(root.parent.parent / name, "openterrainstudio_import/" + name)
print(out)
