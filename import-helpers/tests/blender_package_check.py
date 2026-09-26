# SPDX-License-Identifier: MIT OR Apache-2.0
# Copyright (c) 2026 EllisonDigital
"""Check ZIP registration/operator and packed-image persistence without user preferences."""
from array import array
from pathlib import Path
import sys
import tempfile
from zipfile import ZipFile
import bpy

root = Path(__file__).resolve().parents[1]
fixture = Path(sys.argv[sys.argv.index('--') + 1]).resolve()
with tempfile.TemporaryDirectory(dir=root / '.work') as installed:
    with ZipFile(root / '.work/openterrainstudio-blender.zip') as archive:
        archive.extractall(installed)
    sys.path.insert(0, installed)
    import openterrainstudio_import as addon
    addon.register()
    for manifest in ('build.json', 'png-only.json'):
        bpy.ops.wm.read_factory_settings(use_empty=True)
        assert bpy.ops.import_scene.openterrainstudio_build(filepath=str(fixture / manifest)) == {'FINISHED'}
        before_vertices = {}
        before_masks = {}
        before_pixels = {}
        for obj in bpy.data.objects:
            before_vertices[obj.name] = [tuple(v.co) for v in obj.data.vertices]
            before_masks[obj.name] = {
                attr.name: [v.value for v in attr.data]
                for attr in obj.data.attributes if attr.name.startswith('OTS ') and attr.data_type == 'FLOAT'
            }
        for img in bpy.data.images:
            if 'ots_node' in img:
                assert img.packed_file is not None
                values = array('f', [0]) * len(img.pixels)
                img.pixels.foreach_get(values)
                before_pixels[img.name] = values
        assert len(before_vertices) == 2 and len(before_pixels) == 4
        saved = root / '.work' / (manifest + '.blend')
        bpy.ops.wm.save_as_mainfile(filepath=str(saved))
        bpy.ops.wm.open_mainfile(filepath=str(saved))
        for name, vertices in before_vertices.items():
            obj = bpy.data.objects[name]
            assert [tuple(v.co) for v in obj.data.vertices] == vertices
            for attr, samples in before_masks[name].items():
                assert [v.value for v in obj.data.attributes[attr].data] == samples
        for name, expected in before_pixels.items():
            img = bpy.data.images[name]
            assert img.packed_file is not None and img.is_float
            actual = array('f', [0]) * len(img.pixels)
            img.pixels.foreach_get(actual)
            assert actual == expected, (manifest, name, max(abs(a-b) for a,b in zip(actual, expected)))
        print('BLENDER_PACKAGE_PASS', bpy.app.version_string, manifest, 'all vertices, mask attributes and packed pixels preserved')
    addon.unregister()
