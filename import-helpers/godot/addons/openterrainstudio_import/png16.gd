# SPDX-License-Identifier: MIT OR Apache-2.0
# Copyright (c) 2026 EllisonDigital
extends RefCounted
## Lossless decoder for OTS noninterlaced grayscale16 PNG (all five filters).
## Avoids Godot's ordinary PNG loader, which reduces these images to eight bits.

static func _be32(data: PackedByteArray, offset: int) -> int:
	return (int(data[offset]) << 24) | (int(data[offset + 1]) << 16) | (int(data[offset + 2]) << 8) | int(data[offset + 3])

static func _crc32(data: PackedByteArray) -> int:
	var crc: int = 0xffffffff
	for byte in data:
		crc ^= byte
		for _bit in 8:
			crc = (crc >> 1) ^ (0xedb88320 if crc & 1 else 0)
	return crc ^ 0xffffffff

static func read_image(path: String, resolution: Vector2i) -> Dictionary:
	var data := FileAccess.get_file_as_bytes(path)
	if data.size() < 33 or data.slice(0, 8) != PackedByteArray([137,80,78,71,13,10,26,10]):
		return {"error": "Not a PNG: " + path}
	var offset := 8
	var compressed := PackedByteArray()
	var width := 0
	var height := 0
	var ended := false
	while offset + 12 <= data.size():
		var length := _be32(data, offset)
		var end := offset + length + 12
		if end > data.size():
			return {"error": "Truncated PNG chunk"}
		var kind := data.slice(offset + 4, offset + 8).get_string_from_ascii()
		if _crc32(data.slice(offset + 4, end - 4)) != _be32(data, end - 4):
			return {"error": "PNG chunk checksum mismatch"}
		if width == 0 and kind != "IHDR":
			return {"error": "PNG must start with IHDR"}
		if kind == "IHDR":
			if width != 0 or length != 13:
				return {"error": "Invalid PNG header"}
			width = _be32(data, offset + 8)
			height = _be32(data, offset + 12)
			if Vector2i(width, height) != resolution:
				return {"error": "PNG dimensions do not match build resolution"}
			if data.slice(offset + 16, offset + 21) != PackedByteArray([16,0,0,0,0]):
				return {"error": "Expected noninterlaced 16-bit grayscale PNG from OpenTerrainStudio"}
		elif kind == "IDAT":
			compressed.append_array(data.slice(offset + 8, end - 4))
		elif kind == "IEND":
			ended = true
			break
		elif (data[offset + 4] & 32) == 0:
			return {"error": "Unsupported critical PNG chunk"}
		offset = end
	if not ended or width < 2 or height < 2 or compressed.is_empty():
		return {"error": "Incomplete PNG"}
	var stride := width * 2
	var raw := compressed.decompress((stride + 1) * height, FileAccess.COMPRESSION_DEFLATE)
	if raw.size() != (stride + 1) * height:
		return {"error": "Invalid PNG compression or pixel count"}
	var previous := PackedByteArray()
	previous.resize(stride)
	var values := PackedFloat32Array()
	values.resize(width * height)
	for y in height:
		var start := y * (stride + 1)
		var method := int(raw[start])
		if method > 4:
			return {"error": "Unsupported PNG filter"}
		var row := raw.slice(start + 1, start + 1 + stride)
		for x in stride:
			var a := int(row[x - 2]) if x >= 2 else 0
			var b := int(previous[x])
			var c := int(previous[x - 2]) if x >= 2 else 0
			var prediction := 0
			match method:
				1: prediction = a
				2: prediction = b
				3: prediction = (a + b) >> 1
				4:
					var p := a + b - c
					var pa := absi(p - a)
					var pb := absi(p - b)
					var pc := absi(p - c)
					prediction = a if pa <= pb and pa <= pc else (b if pb <= pc else c)
			row[x] = (int(row[x]) + prediction) & 255
		for x in width:
			values[y * width + x] = float((int(row[x * 2]) << 8) | int(row[x * 2 + 1])) / 65535.0
		previous = row
	return {"image": Image.create_from_data(width, height, false, Image.FORMAT_RF, values.to_byte_array())}
