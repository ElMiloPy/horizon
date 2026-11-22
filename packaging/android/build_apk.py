#!/usr/bin/env python3
"""Build Horizon's arm64-v8a Android APK: horizon-android as a NativeActivity library.

The APK holds the native game library and launcher icons. Game assets and resources
are loaded from the app's internal storage or shared directories.

Usage:
    python3 packaging/android/build_apk.py                 # build library, then package APK
    python3 packaging/android/build_apk.py --no-build      # package existing built library
    python3 packaging/android/build_apk.py --install       # package and adb install

Works both with the standard Android SDK or directly on-device in Termux.
"""

import argparse
import os
import shutil
import subprocess
import sys
import zipfile
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
HERE = Path(__file__).resolve().parent

APPLICATION_ID = "dev.horizon.mobile"
LABEL = "Horizon"
VERSION_NAME = "0.1.0"
VERSION_CODE = 1
MIN_SDK = 26  # AAudio (cpal)
TARGET_SDK = 35
PLATFORM = 35
ABI = "arm64-v8a"
TRIPLE = "aarch64-linux-android"
LIBRARY = "horizon_android"
PAGE_FLAGS = "-C link-arg=-Wl,-z,max-page-size=16384"
ICONS = {
    "mipmap-mdpi": "48x48",
    "mipmap-xxhdpi": "128x128",
    "mipmap-xxxhdpi": "256x256",
}


def sdk_dir():
    env = os.environ.get("ANDROID_HOME") or os.environ.get("ANDROID_SDK_ROOT")
    if env:
        return Path(env)
    if sys.platform == "win32":
        return Path(os.environ.get("LOCALAPPDATA", "")) / "Android" / "Sdk"
    if sys.platform == "darwin":
        return Path.home() / "Library" / "Android" / "sdk"
    return Path.home() / "Android" / "Sdk"


def find_framework():
    """Locate android.jar from SDK or device framework-res.apk."""
    sdk = sdk_dir()
    for plat in (PLATFORM, 36, 35, 34, 33, 32, 31, 30, 29, 28, 26):
        jar = sdk / "platforms" / f"android-{plat}" / "android.jar"
        if jar.exists():
            return jar
    # On Android (e.g. Termux), use the system framework resources
    dev_fw = Path("/system/framework/framework-res.apk")
    if dev_fw.exists():
        return dev_fw
    sys.exit("build_apk: neither android.jar (SDK) nor /system/framework/framework-res.apk found")


def find_tool(name):
    which = shutil.which(name)
    if which:
        return Path(which)
    sdk = sdk_dir()
    build_tools = sdk / "build-tools"
    if build_tools.exists():
        for bt in sorted(build_tools.iterdir(), reverse=True):
            for suffix in ("", ".exe", ".bat", ".cmd"):
                t = bt / (name + suffix)
                if t.is_file():
                    return t
    return None


def run(*args, **kwargs):
    print("+", " ".join(str(a) for a in args), flush=True)
    subprocess.run([str(a) for a in args], check=True, **kwargs)


def build_library(profile):
    env = dict(os.environ)
    env["RUSTFLAGS"] = (env.get("RUSTFLAGS", "") + " " + PAGE_FLAGS).strip()
    # Configure size optimizations via Cargo environment overrides so root Cargo.toml stays clean:
    prof_upper = profile.upper().replace("-", "_")
    env.setdefault(f"CARGO_PROFILE_{prof_upper}_OPT_LEVEL", "z")
    env.setdefault(f"CARGO_PROFILE_{prof_upper}_LTO", "thin")
    env.setdefault(f"CARGO_PROFILE_{prof_upper}_CODEGEN_UNITS", "1")
    env.setdefault(f"CARGO_PROFILE_{prof_upper}_PANIC", "abort")
    env.setdefault(f"CARGO_PROFILE_{prof_upper}_INCREMENTAL", "false")
    # Native compilation in Termux / aarch64 Android environment
    if (sys.platform.startswith("linux") or sys.platform == "android") and os.uname().machine in ("aarch64", "arm64") and not shutil.which("cargo-ndk"):
        run(
            "cargo", "rustc", "--profile", profile, "-p", "horizon-android",
            "--",
            "-C", "link-arg=-Wl,-z,max-page-size=16384",
            "-C", "link-arg=-fuse-ld=lld",
            "-C", "link-arg=-Wl,--icf=all,--gc-sections,--pack-dyn-relocs=android+relr,-s",
            "-C", "link-arg=-Wl,--wrap=open,--wrap=openat,--wrap=stat,--wrap=lstat,--wrap=fstat,--wrap=fstatat,--wrap=access,--wrap=opendir,--wrap=fdopendir,--wrap=readdir,--wrap=closedir,--wrap=dirfd,--wrap=close",
            cwd=ROOT, env=env,
        )
    else:
        run(
            "cargo", "ndk", "-t", ABI, "--platform", str(MIN_SDK),
            "build", "--profile", profile, "-p", "horizon-android",
            cwd=ROOT, env=env,
        )


def find_eaglercraft_html():
    candidates = [
        Path("/sdcard/Download/eaglercraft-26.3-8c167d43-bd5aec75/eaglercraft-26.3-8c167d43.html"),
        Path("/sdcard/Download/eaglercraft-26.3.html"),
    ] + sorted(Path("/sdcard/Download").glob("*eaglercraft*.html"))
    for p in candidates:
        if p.is_file():
            return p
    return None


def parse_epk(raw_b64):
    import base64
    import struct
    import zlib

    data = base64.b64decode(raw_b64.strip())
    gz_idx = data.find(b"\x1f\x8b\x08")
    decomp = zlib.decompress(data[gz_idx:], 16 + zlib.MAX_WBITS)
    pos = 0
    entries = []
    while pos < len(decomp):
        if decomp[pos : pos + 4] == b"FILE":
            pos += 4
            nl = decomp[pos]
            pos += 1
            name = decomp[pos : pos + nl].decode("latin1")
            pos += nl
            cl = struct.unpack(">I", decomp[pos : pos + 4])[0]
            pos += 4
            pos += 4  # CRC
            content = decomp[pos : pos + cl]
            pos += cl
            if pos < len(decomp) and decomp[pos : pos + 1] == b">":
                pos += 1
            entries.append((name, content))
        elif decomp[pos : pos + 4] == b"HEAD":
            pos += 4
            nl = decomp[pos]
            pos += 1
            pos += nl
            vl = struct.unpack(">I", decomp[pos : pos + 4])[0]
            pos += 4
            pos += vl
            if pos < len(decomp) and decomp[pos : pos + 1] == b">":
                pos += 1
        elif decomp[pos : pos + 4] == b"END$":
            break
        else:
            pos += 1
    return entries


def prepare_bundled_resources(out, force=False):
    """Bundle all game assets, sounds, and engine data into solid LZMA assets/resources.hpk.
    Uses official Mojang 26.3 textures, models, blockstates, and matching panoramas, combined
    with an optimized sound footprint and solid LZMA compression to massively beat the HTML version's size."""
    import re, lzma, struct, io

    assets_dir = out / "assets"
    assets_dir.mkdir(parents=True, exist_ok=True)
    res_hpk = assets_dir / "resources.hpk"

    if not force and res_hpk.is_file() and res_hpk.stat().st_size > 20 * 1024 * 1024:
        print(f"+ using existing resources.hpk ({res_hpk.stat().st_size / (1024*1024):.2f} MB)", flush=True)
        return res_hpk

    html_file = find_eaglercraft_html()
    if not html_file:
        sys.exit("build_apk: Eaglercraft HTML file not found in /sdcard/Download/")

    print(f"+ extracting optimized gameplay sounds from {html_file.name}...", flush=True)
    text = html_file.read_text(encoding="utf-8", errors="ignore")

    m_sounds = re.search(r'<script type="application/octet-stream" id="eag-inline-sounds"[^>]*>(.*?)</script>', text, re.DOTALL)
    if not m_sounds:
        sys.exit("build_apk: eag-inline-sounds not found in HTML")
    sounds_entries = parse_epk(m_sounds.group(1))

    print(f"+ creating {res_hpk.name} with solid LZMA compression...", flush=True)
    src = ROOT / ".local" / "horizon-client" / "26.3"

    main_files = []
    sound_files = []
    written = set()

    for f in sorted(src.rglob("*")):
        if not f.is_file():
            continue
        rel = f.relative_to(src).as_posix()
        if rel.startswith("assets/minecraft/sounds/"):
            continue
        bname = rel.encode("utf-8")
        data = f.read_bytes()
        main_files.append((bname, data))
        written.add(rel)

    for name, content in sounds_entries:
        if name not in written:
            bname = name.encode("utf-8")
            sound_files.append((bname, content))
            written.add(name)

    def pack_stream(files):
        buf = io.BytesIO()
        buf.write(struct.pack("<I", len(files)))
        for bname, data in files:
            buf.write(struct.pack("<H", len(bname)))
            buf.write(bname)
            buf.write(struct.pack("<I", len(data)))
            buf.write(data)
        raw = buf.getvalue()
        comp = lzma.compress(raw, preset=9 | lzma.PRESET_EXTREME)
        return comp, len(raw)

    comp_main, raw_main_len = pack_stream(main_files)
    comp_sounds, raw_sounds_len = pack_stream(sound_files)

    hpk_buf = io.BytesIO()
    hpk_buf.write(b"HPK1")
    hpk_buf.write(struct.pack("<I", 2))
    hpk_buf.write(struct.pack("<II", len(comp_main), raw_main_len))
    hpk_buf.write(comp_main)
    hpk_buf.write(struct.pack("<II", len(comp_sounds), raw_sounds_len))
    hpk_buf.write(comp_sounds)

    res_hpk.write_bytes(hpk_buf.getvalue())
    sz = res_hpk.stat().st_size
    print(f"+ created resources.hpk ({sz / (1024*1024):.2f} MB, {len(written)} files)", flush=True)
    return res_hpk


def link_resources(framework, out):
    """Compile manifest and launcher icons into base.apk."""
    manifest = (HERE / "AndroidManifest.xml").read_text(encoding="utf-8")
    manifest = manifest.replace("@APPLICATION_ID@", APPLICATION_ID).replace("@LABEL@", LABEL)
    (out / "AndroidManifest.xml").write_text(manifest, encoding="utf-8")
    res = out / "res"
    shutil.rmtree(res, ignore_errors=True)
    for folder, size in ICONS.items():
        target = res / folder
        target.mkdir(parents=True, exist_ok=True)
        src_icon = ROOT / "packaging" / "icons" / "hicolor" / size / "apps" / "horizon.png"
        if src_icon.exists():
            shutil.copyfile(src_icon, target / "ic_launcher.png")

    aapt2 = find_tool("aapt2")
    if aapt2:
        try:
            compiled = out / "compiled"
            shutil.rmtree(compiled, ignore_errors=True)
            compiled.mkdir()
            run(aapt2, "compile", "--dir", res, "-o", compiled)
            cmd = [
                aapt2, "link", "-o", out / "base.apk",
                "--manifest", out / "AndroidManifest.xml",
                "-I", framework,
                "--min-sdk-version", str(MIN_SDK), "--target-sdk-version", str(TARGET_SDK),
                "--version-code", str(VERSION_CODE), "--version-name", VERSION_NAME,
                "--debug-mode",
            ] + sorted(compiled.glob("*.flat"))
            run(*cmd)
            return
        except Exception as e:
            print(f"aapt2 failed ({e}), falling back to aapt...")

    aapt = find_tool("aapt")
    if not aapt:
        sys.exit("build_apk: neither aapt2 nor aapt found")
    cmd = [
        aapt, "package", "-F", out / "base.apk",
        "-M", out / "AndroidManifest.xml",
        "-S", res,
        "-I", framework,
        "--min-sdk-version", str(MIN_SDK),
        "--target-sdk-version", str(TARGET_SDK),
        "--version-code", str(VERSION_CODE),
        "--version-name", VERSION_NAME,
        "-f",
    ]
    run(*cmd)


def _align_zip_info(zinfo, current_offset, align):
    header_len = 30 + len(zinfo.filename.encode("utf-8"))
    pad = (align - ((current_offset + header_len) % align)) % align
    zinfo.extra = b"\x00" * pad


def add_library_and_align(base_apk, library_so, aligned_apk):
    """Add stripped 16KB-page-aligned .so library and bundled resources.hpk to aligned APK."""
    bundled = ROOT / "target" / "android" / "assets" / "resources.hpk"
    if not bundled.is_file():
        bundled = ROOT / "target" / "android" / "assets" / "resources.zip"

    print("+ packaging and 16KB-page-aligning APK (extractNativeLibs=false)...", flush=True)
    with zipfile.ZipFile(base_apk, "r") as zin, open(aligned_apk, "wb") as fout:
        with zipfile.ZipFile(fout, "w") as zout:
            for item in zin.infolist():
                if "engine-data.zip" in item.filename or "resources.zip" in item.filename or "resources.hpk" in item.filename:
                    continue
                data = zin.read(item.filename)
                info = zipfile.ZipInfo(item.filename)
                info.compress_type = item.compress_type
                info.external_attr = item.external_attr
                if info.compress_type == zipfile.ZIP_STORED:
                    _align_zip_info(info, fout.tell(), 4)
                zout.writestr(info, data)

            if bundled.is_file():
                print(f"+ bundling {bundled.name} ({bundled.stat().st_size / (1024*1024):.2f} MB)...", flush=True)
                with open(bundled, "rb") as fbundle:
                    res_entry = zipfile.ZipInfo(f"assets/{bundled.name}")
                    res_entry.compress_type = zipfile.ZIP_STORED
                    _align_zip_info(res_entry, fout.tell(), 4)
                    zout.writestr(res_entry, fbundle.read())

            so_entry = zipfile.ZipInfo(f"lib/{ABI}/lib{LIBRARY}.so")
            so_entry.compress_type = zipfile.ZIP_STORED
            _align_zip_info(so_entry, fout.tell(), 16384)
            with open(library_so, "rb") as fso:
                zout.writestr(so_entry, fso.read())


def sign(aligned_apk, final_apk, out):
    keystore = os.environ.get("HORIZON_ANDROID_KEYSTORE")
    alias = os.environ.get("HORIZON_ANDROID_KEY_ALIAS", "horizon-debug")
    env = dict(os.environ)
    if not keystore:
        keystore = out / "debug.keystore"
        env["HORIZON_ANDROID_STORE_PASSWORD"] = env["HORIZON_ANDROID_KEY_PASSWORD"] = "android"
        if not keystore.exists():
            keytool = find_tool("keytool") or "keytool"
            run(
                keytool, "-genkeypair", "-keystore", keystore, "-storepass", "android",
                "-keypass", "android", "-alias", alias, "-keyalg", "RSA", "-keysize", "2048",
                "-validity", "10000", "-dname", "CN=Horizon Android debug",
            )
    apksigner = find_tool("apksigner")
    if apksigner:
        run(
            apksigner, "sign",
            "--v1-signing-enabled", "true",
            "--v2-signing-enabled", "true",
            "--v3-signing-enabled", "true",
            "--ks", keystore, "--ks-key-alias", alias,
            "--ks-pass", "env:HORIZON_ANDROID_STORE_PASSWORD",
            "--key-pass", "env:HORIZON_ANDROID_KEY_PASSWORD",
            "--out", final_apk, aligned_apk, env=env,
        )
        run(apksigner, "verify", final_apk)
    else:
        jarsigner = find_tool("jarsigner")
        if not jarsigner:
            sys.exit("build_apk: neither apksigner nor jarsigner found")
        shutil.copyfile(aligned_apk, final_apk)
        run(
            jarsigner, "-keystore", keystore, "-storepass", "android",
            "-keypass", "android", final_apk, alias,
        )


def main():
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--profile", default="fast", help="Cargo profile (default: fast)")
    parser.add_argument("--no-build", action="store_true", help="package existing built library")
    parser.add_argument("--repack-assets", action="store_true", help="force re-extracting assets from HTML")
    parser.add_argument("--install", action="store_true", help="adb install the APK")
    args = parser.parse_args()

    framework = find_framework()
    print(f"Using Android framework resources: {framework}")

    if not args.no_build:
        build_library(args.profile)

    profile_dir = "debug" if args.profile == "dev" else args.profile
    built_candidates = [
        ROOT / "target" / TRIPLE / profile_dir / f"lib{LIBRARY}.so",
        ROOT / "target" / profile_dir / f"lib{LIBRARY}.so",
    ]
    built = next((p for p in built_candidates if p.is_file()), None)
    if not built:
        sys.exit(f"build_apk: lib{LIBRARY}.so not found in target directories")

    out = ROOT / "target" / "android"
    out.mkdir(parents=True, exist_ok=True)
    prepare_bundled_resources(out, force=args.repack_assets)
    library = out / f"lib{LIBRARY}.so"
    strip = find_tool("llvm-strip") or find_tool("strip")
    if strip:
        print(f"+ stripping {library.name} with {strip.name}...", flush=True)
        run(strip, "--strip-all", "-o", library, built)
    else:
        shutil.copyfile(built, library)

    link_resources(framework, out)
    aligned_apk = out / "aligned.apk"
    add_library_and_align(out / "base.apk", library, aligned_apk)

    final_apk = out / f"horizon-{VERSION_NAME}-{ABI}.apk"
    sign(aligned_apk, final_apk, out)

    apk_sz = final_apk.stat().st_size
    html_file = find_eaglercraft_html()
    html_sz = html_file.stat().st_size if html_file else 77552871
    diff = html_sz - apk_sz

    print("\n========================================================")
    print(f"SUCCESS! Built standalone Horizon APK with bundled assets:")
    print(f"  Target APK:            {final_apk}")
    print(f"  Final APK Size:        {apk_sz} bytes ({apk_sz / (1024 * 1024):.2f} MB)")
    print(f"  Eaglercraft 26.3 HTML: {html_sz} bytes ({html_sz / (1024 * 1024):.2f} MB)")
    if diff > 0:
        print(f"  >>> APK BEATS HTML SIZE BY {diff} bytes ({diff / (1024 * 1024):.2f} MB)! <<<")
    else:
        print(f"  APK is {-diff} bytes larger than HTML file.")
    print("========================================================\n")

    download_dir = Path("/sdcard/Download")
    if download_dir.is_dir():
        dest = download_dir / final_apk.name
        shutil.copyfile(final_apk, dest)
        print(f"Copied APK to: {dest}")

    if args.install:
        adb = find_tool("adb")
        if adb:
            run(adb, "install", "-r", final_apk)
        else:
            print("adb not found, skipping install")


if __name__ == "__main__":
    main()
