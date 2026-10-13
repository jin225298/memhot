#!/usr/bin/env python3
"""Sign the reviewed ARM64 release with the user's private, persistent Memhot key."""
import hashlib
import json
import os
import subprocess
import zipfile
from pathlib import Path

from android_release import generate_manifest, write_json


def main():
    root = Path(__file__).resolve().parents[2]
    sdk = Path(os.environ.get("ANDROID_HOME", Path.home() / "Library/Android/sdk"))
    build = sdk / "build-tools/36.0.0"
    ndk = Path(os.environ.get("NDK_HOME", sdk / "ndk/29.0.14206865"))
    readelf = ndk / "toolchains/llvm/prebuilt/darwin-x86_64/bin/llvm-readelf"
    cache = Path.home() / "Library/Caches/memhot-tools"
    cache.mkdir(parents=True, exist_ok=True)
    signing = Path.home() / ".local/share/memhot/signing"
    source = root / "apps/memhot/src-tauri/gen/android/app/build/outputs/apk/universal/release/app-universal-release-unsigned.apk"
    config = json.loads((root / "apps/memhot/src-tauri/tauri.conf.json").read_text())
    version = config["version"]
    version_code = config["bundle"]["android"]["versionCode"]
    target = root / f"artifacts/memhot/memhot-{version}-arm64.apk"
    target.parent.mkdir(parents=True, exist_ok=True)
    aligned = cache / f"memhot-{version}-release-aligned.apk"
    env = os.environ.copy()

    def run(args, log=None):
        if log:
            with (cache / log).open("w") as output:
                subprocess.run(args, check=True, env=env, stdout=output, stderr=subprocess.STDOUT)
        else:
            subprocess.run(args, check=True, env=env)

    run([str(build / "zipalign"), "-f", "-P", "16", "4", str(source), str(aligned)])
    # PKCS12 uses the same key/store password. apksigner otherwise reads a second
    # line when the same file is supplied again via --key-pass.
    run([
        str(build / "apksigner"), "sign", "--ks", str(signing / "memhot-release.p12"),
        "--ks-key-alias", "memhot-release", "--ks-pass", "file:" + str(signing / "keystore-password"),
        "--v4-signing-enabled", "false", "--out", str(target), str(aligned),
    ])
    run([str(build / "apksigner"), "verify", "--verbose", "--print-certs", str(target)], f"apk-{version}-signature-verify.log")
    run([str(build / "zipalign"), "-c", "-P", "16", "-v", "4", str(target)], f"apk-{version}-zipalign-16k.log")
    evidence = []
    with zipfile.ZipFile(target) as apk:
        libraries = [name for name in apk.namelist() if name.startswith("lib/") and name.endswith(".so")]
        assert libraries and all(name.startswith("lib/arm64-v8a/") for name in libraries), libraries
        for name in libraries:
            local = cache / Path(name).name
            local.write_bytes(apk.read(name))
            result = subprocess.run([str(readelf), "-lW", str(local)], check=True, capture_output=True, text=True)
            loads = [line for line in result.stdout.splitlines() if line.lstrip().startswith("LOAD")]
            assert loads and all(int(line.split()[-1], 16) >= 16_384 for line in loads), loads
            evidence.append(name + "\n" + "\n".join(loads))
    (cache / f"apk-{version}-elf-alignment.log").write_text("\n".join(evidence) + "\n")
    badging = subprocess.run([str(build / "aapt"), "dump", "badging", str(target)], check=True, capture_output=True, text=True).stdout
    assert "application-debuggable" not in badging, "Release must not be debuggable"
    assert f"versionName='{version}'" in badging, "Release version name does not match configuration"
    assert f"versionCode='{version_code}'" in badging, "Release version code does not match configuration"
    (cache / f"apk-{version}-badging.log").write_text(badging)
    sha = hashlib.sha256(target.read_bytes()).hexdigest()
    target.with_suffix(".apk.sha256").write_text(sha + "  " + target.name + "\n")
    notes = json.loads((root / f"ops/memhot/android-releases/{version}.json").read_text())
    update = generate_manifest(target, config, notes, badging)
    update_path = target.with_suffix(".update.json")
    write_json(update_path, update)
    print(f"Signed release: {target}\nSize: {target.stat().st_size} bytes\nSHA256: {sha}")
    print(f"Update manifest: {update_path}")
    print("PASS signature / release manifest / ARM64 only / ZIP and ELF 16KiB alignment")


if __name__ == "__main__":
    main()
