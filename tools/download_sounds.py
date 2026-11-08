#!/usr/bin/env python3
"""Download Minecraft sounds and update Horizon resources for Android packaging.

Downloads official sound files from Mojang CDN (resources.download.minecraft.net)
matching .local/horizon-client/26.3/asset-index.json and packages them into
target/android/assets/resources.zip.
"""

import hashlib
import json
import os
import shutil
import sys
import time
import urllib.request
import zipfile
from concurrent.futures import ThreadPoolExecutor, as_completed
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
LOCAL = ROOT / ".local"
HORIZON_RESOURCES = LOCAL / "horizon-client" / "26.3"
INDEX_FILE = HORIZON_RESOURCES / "asset-index.json"
TARGET_ASSETS = ROOT / "target" / "android" / "assets"
RESOURCES_ZIP = TARGET_ASSETS / "resources.zip"


def sha1_file(path: Path) -> str:
    h = hashlib.sha1()
    with open(path, "rb") as f:
        while chunk := f.read(65536):
            h.update(chunk)
    return h.hexdigest()


def download_object(name: str, hash_val: str, expected_size: int, dest: Path) -> bool:
    if dest.is_file() and dest.stat().st_size == expected_size:
        return False
    dest.parent.mkdir(parents=True, exist_ok=True)
    temp = dest.with_suffix(".part")
    url = f"https://resources.download.minecraft.net/{hash_val[:2]}/{hash_val}"
    req = urllib.request.Request(url, headers={"User-Agent": "Horizon/0.1.0"})
    for attempt in range(3):
        try:
            with urllib.request.urlopen(req, timeout=15) as resp, open(temp, "wb") as f:
                while chunk := resp.read(65536):
                    f.write(chunk)
            if temp.stat().st_size != expected_size:
                temp.unlink(missing_ok=True)
                continue
            temp.replace(dest)
            return True
        except Exception as e:
            if attempt == 2:
                print(f"Failed downloading {name} ({url}): {e}", file=sys.stderr)
            time.sleep(0.5)
    return False


def zip_resources(source_dir: Path, zip_path: Path):
    print(f"Packaging {source_dir} -> {zip_path}...")
    zip_path.parent.mkdir(parents=True, exist_ok=True)
    temp_zip = zip_path.with_suffix(".part")
    with zipfile.ZipFile(temp_zip, "w") as z:
        for file in sorted(source_dir.rglob("*")):
            if not file.is_file():
                continue
            rel = file.relative_to(source_dir).as_posix()
            # Store already compressed audio/images uncompressed for speed
            compress = (
                zipfile.ZIP_STORED
                if rel.endswith((".ogg", ".png", ".jpg"))
                else zipfile.ZIP_DEFLATED
            )
            z.write(file, rel, compress_type=compress)
    temp_zip.replace(zip_path)
    print(f"Created {zip_path}: {zip_path.stat().st_size / (1024*1024):.2f} MB")


def main():
    if not INDEX_FILE.is_file():
        sys.exit(f"Missing {INDEX_FILE}. Run tools/download_and_prepare_vanilla.py first.")

    with open(INDEX_FILE, "r", encoding="utf-8") as f:
        data = json.load(f)

    objects = data.get("objects", {})
    sound_items = [
        (k, v["hash"], v["size"])
        for k, v in objects.items()
        if k == "minecraft/sounds.json" or k.startswith("minecraft/sounds/")
    ]
    total_files = len(sound_items)
    total_bytes = sum(s for _, _, s in sound_items)
    print(f"Found {total_files} sound objects ({total_bytes / (1024*1024):.2f} MB).")

    assets_root = HORIZON_RESOURCES / "assets"
    tasks = []
    for rel_name, hash_val, size in sound_items:
        dest = assets_root / rel_name
        tasks.append((rel_name, hash_val, size, dest))

    downloaded = 0
    cached = 0
    start_time = time.time()
    with ThreadPoolExecutor(max_workers=24) as pool:
        futures = {
            pool.submit(download_object, rel, h, s, dest): rel
            for rel, h, s, dest in tasks
        }
        for i, fut in enumerate(as_completed(futures), 1):
            if fut.result():
                downloaded += 1
            else:
                cached += 1
            if i % 500 == 0 or i == total_files:
                elapsed = time.time() - start_time
                print(f"[{i}/{total_files}] Downloaded: {downloaded}, Cached: {cached} ({elapsed:.1f}s)")

    print(f"All sounds ready! Downloaded {downloaded}, cached {cached}.")

    # Also build the target/android/assets/resources.zip package
    zip_resources(HORIZON_RESOURCES, RESOURCES_ZIP)


if __name__ == "__main__":
    main()
