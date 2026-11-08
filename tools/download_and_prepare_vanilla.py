#!/usr/bin/env python3
"""Temporary helper to download official Minecraft 26.3 files and prepare Horizon resources.

Replicates Starling's unauthenticated download logic (piston-meta.mojang.com)
and runs horizon-prepare to generate .local/horizon-client/26.3 without requiring
a Mojang account.
"""

import hashlib
import json
import os
import shutil
import subprocess
import sys
import urllib.request
from concurrent.futures import ThreadPoolExecutor
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
LOCAL = ROOT / ".local"
MINECRAFT_DIR = LOCAL / "minecraft" / "26.3"
OUTPUT_DIR = LOCAL / "horizon-client" / "26.3"
JRE_DIR = LOCAL / "jre25"


def sha1_file(path: Path) -> str:
    h = hashlib.sha1()
    with open(path, "rb") as f:
        while chunk := f.read(65536):
            h.update(chunk)
    return h.hexdigest()


def download_file(url: str, dest: Path, expected_sha1: str = None) -> bool:
    if dest.is_file():
        if not expected_sha1 or sha1_file(dest) == expected_sha1:
            return False
    dest.parent.mkdir(parents=True, exist_ok=True)
    temp = dest.with_suffix(".part")
    req = urllib.request.Request(url, headers={"User-Agent": "Horizon-Android-Probe/0.1.0"})
    with urllib.request.urlopen(req) as resp, open(temp, "wb") as f:
        while chunk := resp.read(65536):
            f.write(chunk)
    if expected_sha1 and sha1_file(temp) != expected_sha1:
        temp.unlink(missing_ok=True)
        raise RuntimeError(f"Hash mismatch downloading {url}")
    temp.replace(dest)
    return True


def ensure_jre25():
    java_bin = JRE_DIR / "bin" / "java"
    if java_bin.is_file():
        return
    print("Fetching OpenJDK 25 for Android ARM64...")
    url = "https://github.com/AngelAuraMC/angelauramc-openjdk-build/releases/download/download_jre25/jre25-android-arm64.tar.xz"
    archive = LOCAL / "jre25.tar.xz"
    LOCAL.mkdir(parents=True, exist_ok=True)
    download_file(url, archive, "d3eb7afe2240c26728a1bb440502c5f18ac3883e932d202dd7f0c9bcbbce4c37")
    import tarfile
    with tarfile.open(archive, "r:xz") as tar:
        tar.extractall(LOCAL)
    archive.unlink(missing_ok=True)
    for name in ["bin", "conf", "legal", "lib", "release"]:
        p = LOCAL / name
        if p.exists() and not (JRE_DIR / name).exists():
            JRE_DIR.mkdir(parents=True, exist_ok=True)
            p.rename(JRE_DIR / name)
    # Ensure javac is a wrapper invoking com.sun.tools.javac.Main
    javac_wrapper = JRE_DIR / "bin" / "javac"
    javac_script = (
        '#!/bin/sh\n'
        'DIR="$(cd "$(dirname "$0")" && pwd)"\n'
        'export LD_LIBRARY_PATH="/data/data/com.termux/files/usr/lib:$DIR/../lib:$DIR/../lib/server:$LD_LIBRARY_PATH"\n'
        'exec "$DIR/java" -m jdk.compiler/com.sun.tools.javac.Main "$@"\n'
    )
    javac_wrapper.write_text(javac_script)
    javac_wrapper.chmod(0o755)


def library_applies(lib: dict) -> bool:
    rules = lib.get("rules")
    if not rules:
        return True
    allowed = False
    for r in rules:
        os_rule = r.get("os", {}).get("name")
        if os_rule is None or os_rule == "linux":
            allowed = (r.get("action") == "allow")
    return allowed


def main():
    with open(ROOT / "research-lock.json") as f:
        lock = json.load(f)["minecraft"]

    version = lock["version"]
    manifest_sha1 = lock["manifest_sha1"]
    client_sha1 = lock["client_sha1"]
    server_sha1 = lock["server_sha1"]

    print(f"=== Downloading Minecraft {version} files from Mojang CDN ===")
    MINECRAFT_DIR.mkdir(parents=True, exist_ok=True)

    # 1. Version JSON
    version_json_path = MINECRAFT_DIR / "versions" / version / f"{version}.json"
    manifest_url = f"https://piston-meta.mojang.com/v1/packages/{manifest_sha1}/{version}.json"
    download_file(manifest_url, version_json_path, manifest_sha1)
    with open(version_json_path) as f:
        manifest = json.load(f)

    # 2. Client & Server JARs
    client_jar_path = MINECRAFT_DIR / "versions" / version / f"{version}.jar"
    client_url = manifest["downloads"]["client"]["url"]
    download_file(client_url, client_jar_path, client_sha1)
    print(f"Client JAR verified: {client_jar_path.stat().st_size / 1024 / 1024:.1f} MB")

    server_jar_path = MINECRAFT_DIR / "server.jar"
    server_url = manifest["downloads"]["server"]["url"]
    download_file(server_url, server_jar_path, server_sha1)
    print(f"Server JAR verified: {server_jar_path.stat().st_size / 1024 / 1024:.1f} MB")

    # Asset Index
    asset_index = manifest.get("assetIndex", {})
    if asset_index:
        idx_url = asset_index["url"]
        idx_id = asset_index.get("id", "34")
        idx_path = MINECRAFT_DIR / "assets" / "indexes" / f"{idx_id}.json"
        download_file(idx_url, idx_path, asset_index.get("sha1"))
        print(f"Asset index verified: {idx_path}")

    # 3. Libraries
    libraries_dir = MINECRAFT_DIR / "libraries"
    downloads = []
    for lib in manifest.get("libraries", []):
        if not library_applies(lib):
            continue
        art = lib.get("downloads", {}).get("artifact")
        if not art:
            continue
        rel_path = art["path"]
        target = libraries_dir / rel_path
        downloads.append((art["url"], target, art["sha1"]))

    print(f"Downloading {len(downloads)} libraries...")
    with ThreadPoolExecutor(max_workers=8) as pool:
        futures = [pool.submit(download_file, url, target, sha1) for url, target, sha1 in downloads]
        for f in futures:
            f.result()
    print("All libraries verified.")

    # 4. Ensure JRE 25
    ensure_jre25()

    # 5. Build/Run horizon-prepare
    print("=== Running horizon-prepare ===")
    prepare_bin = ROOT / "target" / "release" / "horizon-prepare"
    if not prepare_bin.is_file():
        subprocess.run(["cargo", "build", "--release", "-p", "horizon-prepare"], check=True, cwd=ROOT)

    env = dict(os.environ)
    env["JAVA_HOME"] = str(JRE_DIR)
    env["LD_LIBRARY_PATH"] = f"/data/data/com.termux/files/usr/lib:{JRE_DIR / 'lib'}:{JRE_DIR / 'lib' / 'server'}"

    cmd = [
        str(prepare_bin),
        "--prepare",
        "--minecraft-dir", str(MINECRAFT_DIR),
        "--server-jar", str(server_jar_path),
        "--java-home", str(JRE_DIR),
        "--output", str(OUTPUT_DIR),
    ]
    print("+", " ".join(cmd))
    subprocess.run(cmd, env=env, check=True, cwd=ROOT)
    print(f"\nSUCCESS! Horizon resources prepared in: {OUTPUT_DIR}")


if __name__ == "__main__":
    main()
