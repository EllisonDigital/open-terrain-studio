# SPDX-License-Identifier: MIT OR Apache-2.0
# Copyright (c) 2026 EllisonDigital
"""Create small manifest variants beside a real Rust export; never alter its files."""
import copy
import json
from pathlib import Path
import sys
folder = Path(sys.argv[1])
info = json.loads((folder / "build.json").read_text())
variants = {}
png = copy.deepcopy(info)
png["files"] = [f for f in png["files"] if f["format"] == "png16"]
variants["png-only"] = png
single = copy.deepcopy(info)
single["app_version"] = "0.1.0"
single["files"] = info["files"][:1]
variants["single"] = single
masks = copy.deepcopy(info)
masks["files"] = [f for f in info["files"] if f["data"] == "mask"]
variants["masks-only"] = masks
for name, key, value in [("bad-generator", "generator", "OtherApp"), ("bad-orientation", "orientation", "flipped"), ("bad-resolution", "resolution", [128,129])]:
    bad = copy.deepcopy(info)
    bad[key] = value
    variants[name] = bad
missing = copy.deepcopy(info)
missing["files"][0]["file"] = "missing.exr"
variants["missing"] = missing
for name, data in variants.items():
    (folder / (name + ".json")).write_text(json.dumps(data, indent=2) + "\n")

# Known byte patterns independently exercise all PNG predictors in both decoders.
from png_fixture import png16
png16(folder / "filters.png", 9, 11, (0, 1, 2, 3, 4))
corrupt = bytearray((folder / "filters.png").read_bytes())
corrupt[-5] ^= 1
(folder / "corrupt.png").write_bytes(corrupt)
