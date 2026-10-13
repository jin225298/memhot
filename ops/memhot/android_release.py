#!/usr/bin/env python3
"""Generate and validate the fixed Memhot Android update feed from a final APK."""
import argparse
from datetime import datetime, timezone
import hashlib
import json
import os
from pathlib import Path
import re
import subprocess
import zipfile

ORIGIN = "https://memhot.angelkang.com"
FEED_URL = ORIGIN + "/updates/android-arm64.json"
PACKAGE = "com.angelkang.memhot"
ABI = "arm64-v8a"
MIN_SDK = 24
MAX_APK_SIZE = 200 * 1024 * 1024
VERSION = re.compile(r"(?:0|[1-9][0-9]{0,5})\.(?:0|[1-9][0-9]{0,5})\.(?:0|[1-9][0-9]{0,5})\Z")
HASH = re.compile(r"[0-9a-f]{64}\Z")
FIELDS = {"schemaVersion", "packageName", "version", "versionCode", "minSdk", "abi",
          "url", "size", "sha256", "notes", "publishedAt"}


def require(condition, message):
    if not condition:
        raise ValueError(message)


def sha(path):
    digest = hashlib.sha256()
    with Path(path).open("rb") as stream:
        for chunk in iter(lambda: stream.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def apk_name(version):
    require(isinstance(version, str) and len(version) <= 32 and VERSION.fullmatch(version),
            "version must be a numeric major.minor.patch without leading zeroes")
    return f"memhot-{version}-arm64.apk"


def version_code(value):
    require(type(value) is int and 0 < value <= 2_100_000_000, "invalid Android versionCode")
    return value


def release_notes(value):
    require(isinstance(value, list) and 1 <= len(value) <= 8, "notes must contain 1–8 entries")
    require(all(isinstance(note, str) and note == note.strip() and 0 < len(note) <= 300
                and not any(ord(char) < 32 for char in note) for note in value),
            "notes must contain short, nonempty, single-line strings")
    return value


def validate_manifest(data):
    require(isinstance(data, dict) and set(data) == FIELDS, "invalid update manifest schema")
    require(type(data["schemaVersion"]) is int and data["schemaVersion"] == 1,
            "unsupported update schemaVersion")
    require(data["packageName"] == PACKAGE and data["abi"] == ABI, "unexpected package or ABI")
    require(type(data["minSdk"]) is int and data["minSdk"] == MIN_SDK, "unexpected minSdk")
    name = apk_name(data["version"])
    version_code(data["versionCode"])
    require(data["url"] == f"{ORIGIN}/downloads/{name}", "unexpected APK URL")
    require(type(data["size"]) is int and 0 < data["size"] <= MAX_APK_SIZE, "invalid APK size")
    require(isinstance(data["sha256"], str) and HASH.fullmatch(data["sha256"]), "invalid APK sha256")
    release_notes(data["notes"])
    date = data["publishedAt"]
    require(isinstance(date, str) and re.fullmatch(r"\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}Z", date),
            "publishedAt must be an ISO UTC timestamp")
    datetime.fromisoformat(date.replace("Z", "+00:00"))
    return data


def check_badging(badging, version, code):
    package = re.search(r"^package: (.+)$", badging, re.MULTILINE)
    require(package is not None, "APK package metadata missing")
    fields = dict(re.findall(r"(\w+)='([^']*)'", package.group(1)))
    require(fields.get("name") == PACKAGE, "APK packageName does not match release")
    require(fields.get("versionName") == version, "APK versionName does not match release")
    require(fields.get("versionCode") == str(code), "APK versionCode does not match release")
    require(re.search(r"^sdkVersion:'24'$", badging, re.MULTILINE), "APK minSdk must be 24")
    require(re.search(r"^native-code: 'arm64-v8a'\s*$", badging, re.MULTILINE),
            "APK must contain only arm64-v8a native code")
    require("application-debuggable" not in badging, "APK must not be debuggable")


def generate_manifest(apk, config, notes, badging, published_at=None):
    version = config["version"]
    apk_name(version)
    code = version_code(config["bundle"]["android"]["versionCode"])
    require(config["identifier"] == PACKAGE, "configuration packageName mismatch")
    require(isinstance(notes, dict) and set(notes) == {"schemaVersion", "version", "versionCode", "notes"}
            and type(notes["schemaVersion"]) is int and notes["schemaVersion"] == 1,
            "invalid release notes schema")
    require(notes["version"] == version and type(notes["versionCode"]) is int
            and notes["versionCode"] == code, "release notes version metadata mismatch")
    check_badging(badging, version, code)
    with zipfile.ZipFile(apk) as archive:
        native = [name for name in archive.namelist() if name.startswith("lib/") and name.endswith(".so")]
        require(native and all(name.startswith("lib/arm64-v8a/") for name in native),
                "APK native library inventory is not ARM64-only")
    data = {"schemaVersion": 1, "packageName": PACKAGE, "version": version, "versionCode": code,
            "minSdk": MIN_SDK, "abi": ABI, "url": f"{ORIGIN}/downloads/{apk_name(version)}",
            "size": Path(apk).stat().st_size, "sha256": sha(apk), "notes": notes["notes"],
            "publishedAt": published_at or datetime.now(timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ")}
    return validate_manifest(data)


def write_json(path, data):
    Path(path).write_text(json.dumps(data, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    for name in ("apk", "config", "notes", "output"):
        parser.add_argument("--" + name, type=Path, required=True)
    sdk = Path(os.environ.get("ANDROID_HOME", Path.home() / "Library/Android/sdk")) / "build-tools/36.0.0"
    parser.add_argument("--aapt", type=Path, default=sdk / "aapt")
    parser.add_argument("--apksigner", type=Path, default=sdk / "apksigner")
    args = parser.parse_args()
    subprocess.run([str(args.apksigner), "verify", "--verbose", str(args.apk)], check=True)
    badging = subprocess.run([str(args.aapt), "dump", "badging", str(args.apk)],
                             check=True, capture_output=True, text=True).stdout
    data = generate_manifest(args.apk, json.loads(args.config.read_text()),
                             json.loads(args.notes.read_text()), badging)
    write_json(args.output, data)
    print(f"Verified update manifest: {args.output}")


if __name__ == "__main__":
    main()
