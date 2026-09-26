# SPDX-License-Identifier: MIT OR Apache-2.0
# Copyright (c) 2026 EllisonDigital
import copy
import json
from pathlib import Path
import struct
import sys
import tempfile
import unittest
import zlib

HELPERS = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(HELPERS / "blender" / "openterrainstudio_import"))
sys.path.insert(0, str(HELPERS / "unreal"))
from ots_manifest import BuildError, load_build, read_png16, read_points, select_outputs, ORIENTATION
from ots_import_build import expected_hints, unreal_height_m, landscape_layout, plan_build, asset_name, instance_batches


from png_fixture import png16


class ImportTests(unittest.TestCase):
    def setUp(self):
        work = HELPERS / ".work"
        work.mkdir(exist_ok=True)
        self.temp = tempfile.TemporaryDirectory(dir=work)
        self.addCleanup(self.temp.cleanup)
        self.folder = Path(self.temp.name)
        png16(self.folder / "height.png")
        png16(self.folder / "mask.png")
        self.info = {"generator":"OpenTerrainStudio", "app_version":"0.1.0",
                     "world_size_m":[1008.0,504.0], "resolution":[127,127],
                     "height_range_m":[-120.0,2280.0], "cell_size_m":[8.0,4.0],
                     "orientation":ORIENTATION,
                     "files":[{"file":"height.png","node":"n_1","port":"out","format":"png16","data":"heightfield","encoding":"0..65535 = height_range_m min..max"}]}
        self.info["unreal"] = expected_hints(self.info)
        self.path = self.folder / "build.json"
        self.write()

    def write(self):
        self.path.write_text(json.dumps(self.info))

    def test_rust_unreal_hints_reproduce_png_heights(self):
        hints = expected_hints(self.info)
        self.assertEqual(hints["xy_scale"], 800.0)
        f32 = lambda v: struct.unpack("<f",struct.pack("<f",v))[0]
        for value in [0,1,32767,32768,65534,65535]:
            # Exactly the float32 evaluation used by World's denormalise test.
            expected = f32(-120.0 + f32(f32(value / 65535.0) * 2400.0))
            self.assertLess(abs(unreal_height_m(value,hints)-expected), 1e-3)
        # Stronger independent check across every representable pixel.
        for value in range(65536):
            self.assertAlmostEqual(unreal_height_m(value,hints),-120.0+value/65535.0*2400.0,places=10)
        self.info["height_range_m"] = [0.0,512.0]
        self.assertLess(abs(expected_hints(self.info)["z_scale"]-100.0),0.002)

    def test_f32_metadata_spelling_and_narrow_height_span(self):
        self.info["height_range_m"] = [20000.0,20000.1]
        self.info["unreal"] = expected_hints(self.info)
        self.write()
        plan_build(self.path)
        self.assertAlmostEqual(unreal_height_m(65535,self.info["unreal"]),20000.099609375)

    def test_png_all_filters_preserve_all_sixteen_bits(self):
        path = self.folder / "filters.png"
        expected = png16(path,9,11,(0,1,2,3,4))
        width,height,actual = read_png16(path,[9,11])
        self.assertEqual((width,height),(9,11))
        self.assertEqual(actual,expected)
        data = bytearray(path.read_bytes())
        data[-5] ^= 1
        path.write_bytes(data)
        with self.assertRaisesRegex(BuildError,"checksum"):
            read_png16(path)

    def test_single_legacy_and_rectangular_scale(self):
        plan = plan_build(self.path)
        self.assertEqual(len(plan["heightfields"]),1)
        self.assertEqual(plan["masks"],[])
        self.assertEqual(plan["scale_cm"][:2],[800.0,400.0])
        for axis in range(2):
            self.assertEqual((127-1)*plan["scale_cm"][axis]/100,self.info["world_size_m"][axis])

    def test_multiple_outputs_masks_and_alternative_formats(self):
        (self.folder / "height.exr").write_bytes(b"stand-in for metadata-selection test")
        exr = dict(self.info["files"][0], file="height.exr",format="exr32",encoding="metres")
        other = dict(self.info["files"][0], node="n_2")
        mask = {"file":"mask.png","node":"n_3","port":"slope","data":"mask","format":"png16","encoding":"0..65535 = 0..1"}
        self.info["files"].extend([exr,other,mask])
        self.write()
        selected = select_outputs(load_build(self.path))
        self.assertEqual([f["format"] for f in selected],["exr32","png16","png16"])
        plan = plan_build(self.path)
        self.assertEqual(len(plan["heightfields"]),2)
        self.assertEqual(len(plan["masks"]),1)
        self.assertTrue(all(f["format"] == "png16" for f in plan["heightfields"]))
        self.info["files"] = [mask]
        self.write()
        self.assertIsNone(plan_build(self.path)["layout"])

    def test_invalid_metadata_missing_files_and_no_foreign_generator(self):
        for key,value,pattern in [("generator","Gaea","OpenTerrainStudio"), ("generator",None,"OpenTerrainStudio"),
                                   ("orientation","reverse","orientation"), ("resolution",[True,127],"resolution"),
                                   ("cell_size_m",[0,8],"positive"), ("world_size_m",[1,2],"disagree"),
                                   ("height_range_m",[2,1],"max must exceed"), ("files",[],"no files")]:
            with self.subTest(key=key,value=value):
                saved = self.info[key]
                self.info[key] = value
                self.write()
                with self.assertRaisesRegex(BuildError,pattern):
                    load_build(self.path)
                self.info[key] = saved
        self.info["files"][0]["file"] = "gone.png"
        self.write()
        with self.assertRaisesRegex(BuildError,"Missing build file"):
            plan_build(self.path)
        self.info["files"][0]["file"] = "../outside.png"
        self.write()
        with self.assertRaisesRegex(BuildError,"inside the build folder"):
            load_build(self.path)

    def test_duplicate_or_conflicting_output_and_encoding_rejected(self):
        self.info["files"] *= 2
        self.write()
        with self.assertRaisesRegex(BuildError,"Duplicate"):
            load_build(self.path)
        self.info["files"] = self.info["files"][:1]
        self.info["files"][0]["encoding"] = "sRGB"
        self.write()
        with self.assertRaisesRegex(BuildError,"encoding"):
            load_build(self.path)

    def test_unreal_layouts_preserve_resolution(self):
        for res in [128,1009,2017,4033,8129]:
            layout = landscape_layout([res,res])
            self.assertEqual(layout["section_size_quads"]*layout["sections_per_component"]*layout["components"][0]+1,res)
            self.assertLessEqual(layout["components"][0]*layout["components"][1],1024)
        for res in [129,1024,2048]:
            with self.assertRaisesRegex(BuildError,"no automatic resampling"):
                landscape_layout([res,res])

    def test_wrong_unreal_hints_and_png_dimensions_fail(self):
        self.info["unreal"]["z_scale"] = 2400*100/512
        self.write()
        with self.assertRaisesRegex(BuildError,"disagrees"):
            plan_build(self.path)
        self.info["unreal"] = expected_hints(self.info)
        self.write()
        png16(self.folder/"height.png",7,7)
        with self.assertRaisesRegex(BuildError,"dimensions"):
            plan_build(self.path)

    def test_asset_names_do_not_collide_after_sanitising(self):
        self.assertNotEqual(asset_name({"node":"a/b","port":"c"}), asset_name({"node":"a_b","port":"c"}))
        self.assertNotEqual(asset_name({"node":"a","port":"b/c"}), asset_name({"node":"a/b","port":"c"}))

    def test_points_alternative_encodings_and_unreal_frame(self):
        species = ["pine", "birch"]
        rows = [[0, 0, -120, 0, 0.25, 0], [1008, 504, 2280, 90, 4, 1]]
        (self.folder / "points.json").write_text(json.dumps({"format": "ots-points", "version": 1, "species": species, "points": rows}))
        (self.folder / "points.csv").write_text("x,y,z,rotation_deg,scale,species\n0,0,-120,0,0.25,pine\n1008,504,2280,90,4,birch\n")
        entries = [{"file": "points." + fmt, "node": "trees", "port": "points", "data": "PointSet", "format": fmt,
                    "encoding": "metres", "count": 2, "species": species} for fmt in ("csv", "json")]
        self.info["files"].extend(entries)
        self.write()
        loaded = load_build(self.path)
        entries = loaded["files"][-2:]
        self.assertEqual([list(row) for row in read_points(entries[0], loaded)], rows)
        self.assertEqual(read_points(entries[1], loaded), rows)
        self.assertEqual([e["format"] for e in select_outputs(loaded) if e["data"] == "PointSet"], ["csv"])
        self.assertEqual(len(plan_build(self.path)["points"]), 1)
        self.assertEqual(list(instance_batches(entries[0], loaded)),
                         [("pine", [(0, 0, -12000, 0, 0.25)]), ("birch", [(100800, 50400, 228000, 90, 4)])])
        for change, pattern in [({"count": 3}, "count mismatch"), ({"species": ["pine"]}, "unknown species"),
                                ({"data": "Unknown"}, "Unsupported"), ({"encoding": "pixels"}, "Unsupported")]:
            bad = dict(entries[0], **change)
            self.info["files"][-2] = bad
            self.write()
            with self.assertRaisesRegex(BuildError, pattern):
                data = load_build(self.path)
                read_points(data["files"][-2], data)
            self.info["files"][-2] = {key: value for key, value in entries[0].items() if key != "path"}
        self.write()
        for changed, pattern in [(lambda p: p[0].__setitem__(0, -0.000002), "outside"),
                                 (lambda p: p[0].__setitem__(2, float("nan")), "Non-finite"),
                                 (lambda p: p[0].__setitem__(5, 2), "species index")]:
            altered = copy.deepcopy(rows)
            changed(altered)
            (self.folder / "points.json").write_text(json.dumps({"format": "ots-points", "version": 1, "species": species, "points": altered}))
            with self.assertRaisesRegex(BuildError, pattern):
                read_points(entries[1], load_build(self.path))
        (self.folder / "points.json").write_text(json.dumps({"format": "ots-points", "version": 2, "species": species, "points": rows}))
        with self.assertRaisesRegex(BuildError, "version"):
            read_points(entries[1], load_build(self.path))


if __name__ == "__main__":
    unittest.main()
