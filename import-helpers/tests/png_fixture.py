# SPDX-License-Identifier: MIT OR Apache-2.0
# Copyright (c) 2026 EllisonDigital
import struct
import zlib


def png16(path, width=127, height=127, filters=(0,), samples=None):
    """Independent PNG encoder exercising every reconstruction filter."""
    if samples is None:
        samples = [(i * 12347 + 37) % 65536 for i in range(width * height)]
    previous = bytes(width * 2)
    pixels = bytearray()
    for row_index in range(height):
        row = struct.pack(">" + "H" * width, *samples[row_index*width:(row_index+1)*width])
        mode = filters[row_index % len(filters)]
        pixels.append(mode)
        for i, value in enumerate(row):
            a = row[i-2] if i >= 2 else 0
            b = previous[i]
            c = previous[i-2] if i >= 2 else 0
            p = a+b-c
            nearest = min([(abs(p-a),0,a), (abs(p-b),1,b), (abs(p-c),2,c)])[2]
            pixels.append((value - [0,a,b,(a+b)//2,nearest][mode]) % 256)
        previous = row
    def chunk(kind, data):
        return struct.pack(">I",len(data)) + kind + data + struct.pack(">I",zlib.crc32(kind+data))
    path.write_bytes(b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR",struct.pack(">IIBBBBB",width,height,16,0,0,0,0)) + chunk(b"IDAT",zlib.compress(pixels)) + chunk(b"IEND",b""))
    return samples
