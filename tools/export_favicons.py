#!/usr/bin/env python3
"""Export browser favicons from the established Hey Boss raster icon."""
import pathlib
import struct
import subprocess

root = pathlib.Path(__file__).resolve().parents[1] / 'mobile/public'
source = root / 'icons/icon-192.png'
images = []
for size in (16, 32, 48):
    destination = root / f'icons/favicon-{size}.png'
    subprocess.run(['sips', '-z', str(size), str(size), str(source), '--out',
                    str(destination)], check=True, stdout=subprocess.DEVNULL)
    images.append((size, destination.read_bytes()))
offset = 6 + 16 * len(images)
entries = []
for size, data in images:
    entries.append(struct.pack('<BBBBHHII', size, size, 0, 0, 1, 32, len(data), offset))
    offset += len(data)
(root / 'favicon.ico').write_bytes(struct.pack('<HHH', 0, 1, len(images)) +
                                  b''.join(entries) + b''.join(data for _, data in images))
