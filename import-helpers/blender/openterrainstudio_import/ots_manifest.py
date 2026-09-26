# SPDX-License-Identifier: MIT OR Apache-2.0
# Copyright (c) 2026 EllisonDigital
"""Dependency-free build validation and lossless PNG decoding, also used by Unreal."""
import csv
import json
import math
from pathlib import Path
import struct
import zlib

ORIENTATION = "row 0 = world Y 0, column 0 = world X 0"
ENCODINGS = {
    ("heightfield", "exr32"): "metres",
    ("heightfield", "png16"): "0..65535 = height_range_m min..max",
    ("mask", "exr32"): "0..1",
    ("mask", "png16"): "0..65535 = 0..1",
    ("PointSet", "csv"): "metres",
    ("PointSet", "json"): "metres",
}


class BuildError(ValueError):
    """A build cannot be imported without changing its meaning."""


def _pair(info, key, positive=False, integer=False):
    values = info.get(key)
    if not isinstance(values, list) or len(values) != 2:
        raise BuildError(f"{key} must contain two numbers")
    for v in values:
        if isinstance(v, bool) or not isinstance(v, (float, int)) or not math.isfinite(v):
            raise BuildError(f"{key} must contain finite numbers")
        if positive and v <= 0:
            raise BuildError(f"{key} must be positive")
        if integer and (not isinstance(v, int) or not 2 <= v <= 16384):
            raise BuildError(f"{key} must contain integers from 2 to 16384")
    return values


def load_build(filename):
    path = Path(filename).expanduser().resolve()
    try:
        info = json.loads(path.read_text(encoding="utf-8"))
    except (OSError, ValueError) as exc:
        raise BuildError(f"Cannot read build.json: {exc}") from exc
    if not isinstance(info, dict) or info.get("generator") != "OpenTerrainStudio":
        raise BuildError("Not an OpenTerrainStudio build: generator must be 'OpenTerrainStudio'")
    size = _pair(info, "world_size_m", positive=True)
    height = _pair(info, "height_range_m")
    res = _pair(info, "resolution", integer=True)
    cell = _pair(info, "cell_size_m", positive=True)
    if height[1] <= height[0]:
        raise BuildError("height_range_m max must exceed min")
    if info.get("orientation") != ORIENTATION:
        raise BuildError("Unsupported or missing orientation; expected " + ORIENTATION)
    for axis in range(2):
        if not math.isclose(cell[axis] * (res[axis] - 1), size[axis], rel_tol=1e-6, abs_tol=1e-6):
            raise BuildError("cell_size_m, world_size_m and resolution disagree")
    files = info.get("files")
    if not isinstance(files, list) or not files:
        raise BuildError("build.json has no files to import")
    seen, types = set(), {}
    for entry in files:
        if not isinstance(entry, dict):
            raise BuildError("Each files entry must be an object")
        for key in ("node", "port", "file"):
            if not isinstance(entry.get(key), str) or not entry[key]:
                raise BuildError(f"Each files entry needs a nonempty {key}")
        key = (entry["node"], entry["port"])
        data, fmt = entry.get("data"), entry.get("format")
        if not isinstance(data, str) or not isinstance(fmt, str):
            raise BuildError("Each files entry needs string data and format")
        expected = ENCODINGS.get((data, fmt))
        if expected is None or entry.get("encoding") != expected:
            raise BuildError(f"Unsupported data/format/encoding for {entry['file']}")
        if data == "PointSet":
            if isinstance(entry.get("count"), bool) or not isinstance(entry.get("count"), int) or entry["count"] < 0:
                raise BuildError("PointSet count must be a nonnegative integer")
            species = entry.get("species")
            if not isinstance(species, list) or any(not isinstance(s, str) or not s or "," in s for s in species) or len(set(species)) != len(species):
                raise BuildError("PointSet species must be unique nonempty string ids")
        if key in types and types[key] != data:
            raise BuildError(f"Conflicting data types for {key}")
        types[key] = data
        identity = (*key, fmt)
        if identity in seen:
            raise BuildError(f"Duplicate output format: {identity}")
        seen.add(identity)
        relative = Path(entry["file"])
        source = (path.parent / relative).resolve()
        if relative.is_absolute() or "\\" in entry["file"] or not source.is_relative_to(path.parent):
            raise BuildError(f"File must be inside the build folder: {entry['file']}")
        if not source.is_file():
            raise BuildError(f"Missing build file: {source}")
        entry["path"] = str(source)
    info["manifest_path"] = str(path)
    return info


def select_outputs(info, preference=("exr32", "png16", "csv", "json")):
    """Preserve manifest order, deduplicating alternative encodings of one output."""
    groups = {}
    for entry in info["files"]:
        groups.setdefault((entry["node"], entry["port"]), []).append(entry)
    return [min(group, key=lambda f: preference.index(f["format"])) for group in groups.values()]


def read_points(entry, info):
    """Return (x, y, z, degrees, scale, species index) rows in manifest order."""
    if entry["data"] != "PointSet":
        raise BuildError("Expected a PointSet entry")
    species = entry["species"]
    try:
        if entry["format"] == "csv":
            with open(entry["path"], newline="", encoding="utf-8") as source:
                rows = csv.reader(source, quoting=csv.QUOTE_NONE)
                if next(rows, None) != ["x", "y", "z", "rotation_deg", "scale", "species"]:
                    raise BuildError("Invalid PointSet CSV header")
                points = []
                indices = {name: i for i, name in enumerate(species)}
                for row in rows:
                    if len(row) != 6 or row[5] not in indices:
                        raise BuildError("Invalid PointSet CSV row or unknown species")
                    points.append((*map(float, row[:5]), indices[row[5]]))
        elif entry["format"] == "json":
            data = json.loads(Path(entry["path"]).read_text(encoding="utf-8"))
            if not isinstance(data, dict) or data.get("format") != "ots-points" or type(data.get("version")) is not int or data["version"] != 1:
                raise BuildError("Unsupported PointSet format or version")
            if data.get("species") != species or not isinstance(data.get("points"), list):
                raise BuildError("PointSet species/points disagree with manifest")
            points = data["points"]
        else:
            raise BuildError("Unsupported PointSet format")
    except (OSError, ValueError, TypeError, OverflowError) as exc:
        if isinstance(exc, BuildError):
            raise
        raise BuildError(f"Cannot read PointSet: {exc}") from exc
    if len(points) != entry["count"]:
        raise BuildError("PointSet count mismatch")
    sx, sy = info["world_size_m"]
    for row in points:
        if not isinstance(row, (list, tuple)) or len(row) != 6:
            raise BuildError("Invalid PointSet point")
        if any(isinstance(v, bool) or not isinstance(v, (int, float)) or not math.isfinite(v) for v in row[:5]):
            raise BuildError("Non-finite or invalid PointSet number")
        if not -1e-6 <= row[0] <= sx + 1e-6 or not -1e-6 <= row[1] <= sy + 1e-6:
            raise BuildError("PointSet point outside world bounds")
        if row[4] <= 0:
            raise BuildError("PointSet scale must be positive")
        if type(row[5]) is not int or not 0 <= row[5] < len(species):
            raise BuildError("Unknown PointSet species index")
    return points


def label(entry):
    # A JSON pair is unambiguous even if node/port names contain separators.
    return json.dumps([entry["node"], entry["port"]], ensure_ascii=False)


def read_png16(filename, resolution=None):
    """Read OTS grayscale16, noninterlaced PNG, ignoring colour/gamma transforms.

    All five PNG row filters are supported. Returns top-row-first uint16 values.
    No Pillow, NumPy, Blender colour management, or Unreal texture quantisation.
    """
    try:
        data = Path(filename).read_bytes()
    except OSError as exc:
        raise BuildError(f"Cannot read PNG: {exc}") from exc
    if data[:8] != b"\x89PNG\r\n\x1a\n":
        raise BuildError("Not a PNG: " + str(filename))
    offset, compressed, dimensions, ended = 8, bytearray(), None, False
    while offset + 12 <= len(data):
        length = struct.unpack_from(">I", data, offset)[0]
        kind = data[offset + 4:offset + 8]
        end = offset + 12 + length
        if end > len(data):
            raise BuildError("Truncated PNG chunk")
        payload = data[offset + 8:end - 4]
        if zlib.crc32(kind + payload) != struct.unpack_from(">I", data, end - 4)[0]:
            raise BuildError("PNG chunk checksum mismatch")
        if dimensions is None and kind != b"IHDR":
            raise BuildError("PNG must start with IHDR")
        if kind == b"IHDR":
            if dimensions is not None or length != 13:
                raise BuildError("Invalid PNG header")
            w, h, depth, colour, compression, filtering, interlace = struct.unpack(">IIBBBBB", payload)
            if not (2 <= w <= 16384 and 2 <= h <= 16384):
                raise BuildError("PNG dimensions must be 2..16384")
            if (depth, colour, compression, filtering, interlace) != (16, 0, 0, 0, 0):
                raise BuildError("Expected noninterlaced 16-bit grayscale PNG from OpenTerrainStudio")
            dimensions = (w, h)
        elif kind == b"IDAT":
            compressed.extend(payload)
        elif kind == b"IEND":
            ended = True
            break
        elif not kind[0] & 32:
            raise BuildError("Unsupported critical PNG chunk")
        offset = end
    if dimensions is None or not ended or not compressed:
        raise BuildError("Incomplete PNG")
    w, h = dimensions
    if resolution is not None and list(dimensions) != list(resolution):
        raise BuildError(f"PNG dimensions {dimensions} do not match build resolution {resolution}")
    stride = w * 2
    expected = (stride + 1) * h
    decoder = zlib.decompressobj()
    try:
        raw = decoder.decompress(compressed, expected + 1)
    except zlib.error as exc:
        raise BuildError(f"Invalid PNG compression: {exc}") from exc
    if len(raw) != expected or not decoder.eof or decoder.unused_data:
        raise BuildError("PNG pixel data length mismatch")
    previous = bytearray(stride)
    samples = []
    for y in range(h):
        start = y * (stride + 1)
        method = raw[start]
        if method > 4:
            raise BuildError("Unsupported PNG filter")
        row = bytearray(raw[start + 1:start + 1 + stride])
        for x in range(stride):
            a = row[x - 2] if x >= 2 else 0
            b = previous[x]
            c = previous[x - 2] if x >= 2 else 0
            predictor = 0
            if method == 1:
                predictor = a
            elif method == 2:
                predictor = b
            elif method == 3:
                predictor = (a + b) // 2
            elif method == 4:
                p = a + b - c
                pa, pb, pc = abs(p - a), abs(p - b), abs(p - c)
                predictor = a if pa <= pb and pa <= pc else b if pb <= pc else c
            row[x] = (row[x] + predictor) & 255
        samples.extend(struct.unpack(">" + "H" * w, row))
        previous = row
    return w, h, samples
