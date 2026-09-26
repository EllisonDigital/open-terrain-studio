File contract. The v0.7 exporter will write exactly this. Copy it word for word into import-helpers/points-format.md.

- In build.json, a files entry for points looks like:
  {"file":"pine_points.csv","node":"<id>","port":"points","data":"PointSet","format":"csv"|"json","encoding":"metres","count":<int>,"species":["scots_pine", ...]}.
  It has no data_min/data_max. Existing validators must accept this entry, and they must still reject unknown data types as they do today.
- Coordinates use the same frame as the images (orientation: "row 0 = world Y 0, column 0 = world X 0").
  - x and y are metres from the world corner, in 0..world_size_m.
  - z is absolute height in metres, the same unit as the exported heights.
  - rotation_deg is rotation about the vertical axis in degrees. 0° points along +X and 90° along +Y.
  - scale is a uniform multiplier where 1.0 means the species' nominal size.
- CSV: a header row x,y,z,rotation_deg,scale,species, then one point per row. species is the string id. Numbers are in shortest round-trip form and there are no quoted fields.
- JSON: {"format":"ots-points","version":1,"species":["scots_pine",...],"points":[[x,y,z,rotation_deg,scale,species_index],...]}.
- Both encodings can be present for the same (node, port). Treat them as alternatives, the same way the helpers already treat EXR and PNG.
