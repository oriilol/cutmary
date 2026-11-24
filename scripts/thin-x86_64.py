#!/usr/bin/env python3
"""
Thin universal (fat) Mach-O binaries inside a macOS .app bundle to x86_64 only.
Preserves file permissions, skips symlinks and already-thin binaries, and cuts
CapCut.app's footprint by ~1.12 GB across 236 universal Mach-O binaries.
"""

import argparse
import os
import struct
import sys

FAT_MAGIC = 0xCAFEBABE
FAT_MAGIC_64 = 0xCAFEBABF
CPU_TYPE_X86_64 = 0x01000007


def thin_macho(path: str, dry_run: bool = False) -> tuple[bool, int, int]:
    try:
        st = os.lstat(path)
    except OSError:
        return (False, 0, 0)

    if os.path.islink(path) or not os.path.isfile(path) or st.st_size < 28:
        return (False, 0, 0)

    with open(path, "rb") as f:
        header = f.read(8)
        if len(header) < 8:
            return (False, 0, 0)
        magic, nfat = struct.unpack(">II", header)
        if magic not in (FAT_MAGIC, FAT_MAGIC_64) or nfat == 0 or nfat > 16:
            return (False, 0, 0)

        entry_fmt = ">IIIII" if magic == FAT_MAGIC else ">IIQQII"
        entry_size = struct.calcsize(entry_fmt)
        table = f.read(nfat * entry_size)
        if len(table) < nfat * entry_size:
            return (False, 0, 0)

        x86_slice = None
        for i in range(nfat):
            fields = struct.unpack_from(entry_fmt, table, i * entry_size)
            cputype = fields[0]
            offset = fields[2]
            size = fields[3]
            if cputype == CPU_TYPE_X86_64:
                x86_slice = (offset, size)
                break

        if x86_slice is None:
            return (False, 0, 0)

        off, size = x86_slice
        if off + size > st.st_size:
            return (False, 0, 0)

        if dry_run:
            return (True, st.st_size, size)

        f.seek(off)
        slice_bytes = f.read(size)

    tmp_path = f"{path}.thin.tmp"
    with open(tmp_path, "wb") as out:
        out.write(slice_bytes)
    os.chmod(tmp_path, st.st_mode & 0o7777)
    os.replace(tmp_path, path)
    return (True, st.st_size, size)


def main() -> int:
    parser = argparse.ArgumentParser(description="Strip non-x86_64 slices from universal Mach-O binaries.")
    parser.add_argument("bundle", help="Path to CapCut.app directory")
    parser.add_argument("--dry-run", action="store_true", help="Report savings without modifying files")
    args = parser.parse_args()

    if not os.path.isdir(args.bundle):
        print(f"error: directory not found: {args.bundle}", file=sys.stderr)
        return 1

    count = 0
    total_before = 0
    total_after = 0

    for root, _dirs, files in os.walk(args.bundle):
        for name in files:
            full = os.path.join(root, name)
            changed, before, after = thin_macho(full, dry_run=args.dry_run)
            if changed:
                count += 1
                total_before += before
                total_after += after

    saved = total_before - total_after
    mode = "[dry-run] " if args.dry_run else ""
    print(
        f"{mode}Thinned {count} universal Mach-O binaries: "
        f"{total_before / (1024**2):.1f} MiB -> {total_after / (1024**2):.1f} MiB "
        f"(saved {saved / (1024**2):.1f} MiB)"
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
