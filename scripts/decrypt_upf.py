#!/usr/bin/env python3
"""Decrypt Sinowealth UPF firmware files.

Uses modified TEA (Tiny Encryption Algorithm) with:
- 32 rounds
- Delta = 0x9E3769B9 (non-standard)
- 4-byte key (each byte zero-extended to 32-bit key word)
- Key stored at end of file, after firmware chunks
"""

import struct
import sys

DELTA = 0x9E3769B9
MASK32 = 0xFFFFFFFF


def tea_decrypt_block(v0, v1, key):
    """Decrypt one 8-byte block (two uint32s) with modified TEA."""
    k0, k1, k2, k3 = key
    s = (DELTA * 32) & MASK32  # initial sum = 0xC6ED3720
    for _ in range(32):
        v1 = (v1 - ((((v0 >> 5) + k3) ^ (s + v0) ^ ((v0 << 4) + k2)) & MASK32)) & MASK32
        v0 = (v0 - ((((v1 >> 5) + k1) ^ (s + v1) ^ ((v1 << 4) + k0)) & MASK32)) & MASK32
        s = (s - DELTA) & MASK32
    return v0, v1


def tea_decrypt(data, key):
    """Decrypt a byte buffer with modified TEA, big-endian block order."""
    out = bytearray(data)
    n_blocks = len(data) // 8
    for i in range(n_blocks):
        off = i * 8
        v0, v1 = struct.unpack_from('>II', out, off)
        v0, v1 = tea_decrypt_block(v0, v1, key)
        struct.pack_into('>II', out, off, v0, v1)
    return bytes(out)


def parse_upf(filepath):
    """Parse UPF header and decrypt firmware chunks."""
    data = open(filepath, 'rb').read()
    print(f"File: {filepath}")
    print(f"Size: {len(data)} bytes")

    # Verify magic
    if data[0:2] != b'\x5a\xa5':
        print(f"ERROR: bad magic {data[0:2].hex()}, expected 5aa5")
        sys.exit(1)

    vendor = data[2:9].decode('ascii', errors='replace')
    fw_type = data[9]
    model = data[10:17].decode('ascii', errors='replace').rstrip('\x00')
    vid = struct.unpack_from('>H', data, 17)[0]
    pid = struct.unpack_from('>H', data, 19)[0]

    print(f"Vendor:  {vendor}")
    print(f"Type:    {fw_type}")
    print(f"Model:   {model}")
    print(f"VID:     0x{vid:04x}")
    print(f"PID:     0x{pid:04x}")

    # Sizes (big-endian uint32)
    size1 = struct.unpack_from('>I', data, 38)[0]
    size2 = struct.unpack_from('>I', data, 42)[0]
    size3 = struct.unpack_from('>I', data, 60)[0] if fw_type == 3 else 0

    print(f"Chunk sizes: {size1}, {size2}, {size3}")

    # Determine chunk count based on type
    if fw_type == 0:
        sizes = [size1, size2]
    elif fw_type == 1:
        sizes = [size1]
    elif fw_type == 2:
        sizes = [size2]
    elif fw_type == 3:
        sizes = [size1, size2, size3]
    else:
        print(f"ERROR: unknown firmware type {fw_type}")
        sys.exit(1)

    # Verify file size
    expected = 0x80 + sum(sizes) + 4
    if expected != len(data):
        print(f"WARNING: expected {expected} bytes, got {len(data)}")

    # Extract key (4 bytes after all chunks)
    key_offset = 0x80 + sum(sizes)
    key_bytes = data[key_offset:key_offset + 4]
    key = tuple(b for b in key_bytes)  # each byte as uint32
    print(f"Key:     {key_bytes.hex()} ({key})")
    print()

    # Decrypt each chunk
    offset = 0x80
    for i, size in enumerate(sizes):
        chunk = data[offset:offset + size]
        print(f"Chunk {i}: offset=0x{offset:x}, size={size} (0x{size:x})")

        decrypted = tea_decrypt(chunk, key)

        # Check for 8051 signatures
        if decrypted[0] == 0x02:
            target = (decrypted[1] << 8) | decrypted[2]
            print(f"  -> Starts with LJMP 0x{target:04x} (8051 reset vector)")
        elif decrypted[0] == 0x00:
            print(f"  -> Starts with NOP")
        elif decrypted[0] == 0xFF:
            print(f"  -> Starts with 0xFF (empty/erased flash)")

        # Show first 64 bytes
        print(f"  First 64 bytes:")
        for j in range(0, 64, 16):
            hx = ' '.join(f'{b:02x}' for b in decrypted[j:j + 16])
            asc = ''.join(chr(b) if 0x20 <= b < 0x7f else '.' for b in decrypted[j:j + 16])
            print(f"    {j:04x}: {hx}  {asc}")

        # Write decrypted chunk
        out_path = filepath.rsplit('.', 1)[0] + f'_chunk{i}.bin'
        open(out_path, 'wb').write(decrypted)
        print(f"  Written to {out_path}")
        print()

        offset += size


if __name__ == '__main__':
    if len(sys.argv) < 2:
        print(f"Usage: {sys.argv[0]} <firmware.upf>", file=sys.stderr)
        sys.exit(1)
    parse_upf(sys.argv[1])
