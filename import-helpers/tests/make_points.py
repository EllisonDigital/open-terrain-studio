# SPDX-License-Identifier: MIT OR Apache-2.0
# Copyright (c) 2026 EllisonDigital
"""Add deterministic PointSet alternatives to a real exported fixture.

Usage: python make_points.py import-helpers/.work/fixture [--million]
Writes points-build.json (or points-million.json) without modifying build.json.
"""
import argparse
import csv
import json
from pathlib import Path
import random
import struct


def make(folder, million=False):
    info = json.loads((folder / "build.json").read_text(encoding="utf-8"))
    count = 1_000_000 if million else 5_000
    stem = "points-million" if million else "points"
    species = ["scots_pine", "silver_birch"]
    rng = random.Random(307)
    sx, sy = info["world_size_m"]
    width, height = info["resolution"]
    raw = folder / "terrain_a_out.f32"
    heights = struct.unpack("<" + "f" * (width * height), raw.read_bytes())
    # Exact grid coordinates make the frame/height check independent of interpolation.
    points = []
    edges = [(0, 0, 0, 0.25, 0), (width - 1, height - 1, 90, 4, 1),
             (0, height - 1, 180, 1, 0), (width - 1, 0, 359.999, 1, 1)]
    for i in range(count):
        if i < len(edges):
            col, row, yaw, scale, index = edges[i]
        else:
            col, row = rng.randrange(width), rng.randrange(height)
            yaw, scale, index = rng.random() * 360, 0.25 + rng.random() * 3.75, i % 2
        points.append([col * info["cell_size_m"][0], row * info["cell_size_m"][1],
                       heights[row * width + col], yaw, scale, index])
    with (folder / (stem + ".csv")).open("w", newline="", encoding="utf-8") as output:
        writer = csv.writer(output, lineterminator="\n")
        writer.writerow(["x", "y", "z", "rotation_deg", "scale", "species"])
        writer.writerows([*row[:5], species[row[5]]] for row in points)
    (folder / (stem + ".json")).write_text(json.dumps({"format": "ots-points", "version": 1,
                                                         "species": species, "points": points}, separators=(",", ":")))
    info["files"].extend({"file": stem + "." + fmt, "node": "vegetation", "port": "points",
                          "data": "PointSet", "format": fmt, "encoding": "metres",
                          "count": count, "species": species} for fmt in ("csv", "json"))
    (folder / (stem + "-build.json")).write_text(json.dumps(info, indent=2) + "\n")
    return folder / (stem + "-build.json")


if __name__ == "__main__":
    parser = argparse.ArgumentParser()
    parser.add_argument("folder", type=Path)
    parser.add_argument("--million", action="store_true")
    args = parser.parse_args()
    print(make(args.folder, args.million))
