#!/usr/bin/env python3
"""Publish an operator-reviewed private Android stage; expose the update feed last."""
import argparse
from contextlib import contextmanager
import fcntl
import json
import os
from pathlib import Path
import shutil
import stat
import subprocess
import tempfile
import time

from android_release import FEED_URL, HASH, ORIGIN, apk_name, require, sha, validate_manifest, write_json

DEPLOYMENTS = Path("/opt/memhot/deployments")
CONFIG = Path("/etc/nginx/sites-available/memhot.angelkang.com")
PUBLIC = Path("/opt/memhot/public")
STAGE_FIELDS = {"schemaVersion", "version", "versionCode", "apkSha256", "oldNginxSha256",
                "newNginxSha256", "updateSha256", "oldFeedSha256"}


def operator_path(path, directory=False, private=False):
    info = path.lstat()
    expected_type = stat.S_ISDIR if directory else stat.S_ISREG
    require(expected_type(info.st_mode) and info.st_uid == 0, f"root-owned regular path required: {path}")
    require(not info.st_mode & (0o077 if private else 0o022), f"unsafe permissions: {path}")


def digest_field(value, name, nullable=False):
    require((nullable and value is None) or (isinstance(value, str) and HASH.fullmatch(value)),
            f"invalid {name}")


def load_json(path, limit=16_384):
    require(path.stat().st_size <= limit, f"JSON file too large: {path.name}")
    return json.loads(path.read_text(encoding="utf-8"))


def sync_dir(path):
    descriptor = os.open(path, os.O_RDONLY | os.O_DIRECTORY)
    try:
        os.fsync(descriptor)
    finally:
        os.close(descriptor)


def atomic_copy(source, target, mode=0o644, immutable=False):
    # Temporary files stay on the destination filesystem for atomic replacement.
    descriptor, name = tempfile.mkstemp(prefix="." + target.name + ".", dir=target.parent)
    temporary = Path(name)
    try:
        with os.fdopen(descriptor, "wb") as output, source.open("rb") as incoming:
            shutil.copyfileobj(incoming, output)
            output.flush()
            os.fchmod(output.fileno(), mode)
            os.fsync(output.fileno())
        if immutable:
            try:
                os.link(temporary, target)
            except FileExistsError:
                operator_path(target)
                require(sha(target) == sha(source), f"published version is immutable: {target.name}")
            temporary.unlink()
        else:
            temporary.replace(target)
        sync_dir(target.parent)
    finally:
        temporary.unlink(missing_ok=True)


def receipt(stage, name, data):
    temporary = stage / ("." + name)
    write_json(temporary, data)
    temporary.chmod(0o600)
    atomic_copy(temporary, stage / name, mode=0o600)
    temporary.unlink()


def command(*args):
    return subprocess.run(args, check=True, capture_output=True, text=True).stdout.strip()


def reload_nginx():
    command("nginx", "-t")
    command("systemctl", "reload", "nginx")


def download(url, destination):
    # No --location: feed, APK, and checksum must each be direct HTTPS 200s.
    status = command("curl", "--fail", "--silent", "--show-error", "--proto", "=https",
                     "--connect-timeout", "15", "--max-time", "180", "--max-redirs", "0",
                     "--output", str(destination), "--write-out", "%{http_code}", url)
    require(status == "200", f"expected direct HTTPS 200, got {status}: {url}")
    destination.chmod(0o600)


def await_static_route(url):
    # systemctl reload returns before new Nginx workers necessarily accept
    # connections. The old worker can still return 404 for a newly added path.
    # Probe only this read-only route; never retry a publication transaction.
    for attempt in range(6):
        status = command("curl", "--head", "--silent", "--show-error", "--proto", "=https",
                         "--connect-timeout", "3", "--max-time", "5", "--max-redirs", "0",
                         "--output", "/dev/null", "--write-out", "%{http_code}", url)
        if status == "200":
            return
        require(status == "404", f"unexpected static route status: {status}")
        if attempt < 5:
            time.sleep(1)
    raise ValueError("new static route did not become ready after Nginx reload")


@contextmanager
def publication_lock():
    operator_path(DEPLOYMENTS, directory=True)
    lock_path = DEPLOYMENTS / ".android-publish.lock"
    descriptor = os.open(lock_path, os.O_CREAT | os.O_RDWR | os.O_NOFOLLOW, 0o600)
    try:
        operator_path(lock_path, private=True)
        try:
            fcntl.flock(descriptor, fcntl.LOCK_EX | fcntl.LOCK_NB)
        except BlockingIOError as error:
            raise ValueError("another Android publication is active") from error
        yield
    finally:
        os.close(descriptor)


def validate_stage(stage):
    require(stage.is_absolute() and stage.parent == DEPLOYMENTS and stage.resolve() == stage,
            "stage must be a direct private directory under /opt/memhot/deployments")
    operator_path(stage, directory=True, private=True)
    for name in ("manifest.json", "nginx.conf", "android-arm64.json"):
        operator_path(stage / name)
    manifest = load_json(stage / "manifest.json")
    require(isinstance(manifest, dict) and set(manifest) == STAGE_FIELDS, "invalid stage manifest schema")
    require(type(manifest["schemaVersion"]) is int and manifest["schemaVersion"] == 1,
            "unsupported stage schemaVersion")
    for field in ("apkSha256", "oldNginxSha256", "newNginxSha256", "updateSha256", "oldFeedSha256"):
        digest_field(manifest[field], field, nullable=field == "oldFeedSha256")
    update = validate_manifest(load_json(stage / "android-arm64.json"))
    require(type(manifest["versionCode"]) is int and manifest["version"] == update["version"]
            and manifest["versionCode"] == update["versionCode"], "stage version metadata mismatch")
    name = apk_name(manifest["version"])
    apk, checksum = stage / name, stage / (name + ".sha256")
    operator_path(apk)
    operator_path(checksum)
    require(sha(apk) == manifest["apkSha256"] == update["sha256"], "APK checksum mismatch")
    require(apk.stat().st_size == update["size"], "APK size mismatch")
    require(checksum.read_text() == update["sha256"] + "  " + name + "\n", "checksum file mismatch")
    require(sha(stage / "nginx.conf") == manifest["newNginxSha256"], "candidate Nginx checksum mismatch")
    require(sha(stage / "android-arm64.json") == manifest["updateSha256"], "update checksum mismatch")
    nginx = (stage / "nginx.conf").read_text()
    for route in (f"/downloads/{name}", f"/downloads/{name}.sha256", "/updates/android-arm64.json"):
        require(nginx.count(f"location = {route} {{") == 1
                and nginx.count(f"alias /opt/memhot/public{route};") == 1,
                f"candidate lacks an exact static route: {route}")
    require(not (stage / "old-nginx.conf").exists(), "stage already used; prepare a fresh private stage")
    return manifest, update, name


def check_active(manifest, update):
    operator_path(CONFIG)
    require(sha(CONFIG) == manifest["oldNginxSha256"], "active Nginx config changed")
    feed = PUBLIC / "updates/android-arm64.json"
    if manifest["oldFeedSha256"] is None:
        require(not feed.exists() and not feed.is_symlink(), "unexpected active update feed")
    else:
        operator_path(feed)
        require(sha(feed) == manifest["oldFeedSha256"], "active update feed changed")
        old = validate_manifest(load_json(feed))
        require(old["versionCode"] < update["versionCode"], "update feed must increase versionCode")
    return feed


def verify_public(stage, update, name):
    downloaded = stage / "public-apk.verify"
    download(update["url"], downloaded)
    require(downloaded.stat().st_size == update["size"] and sha(downloaded) == update["sha256"],
            "public APK size or checksum mismatch")
    downloaded.unlink()
    checksum = stage / "public-sha256.verify"
    download(update["url"] + ".sha256", checksum)
    require(checksum.read_bytes() == (stage / (name + ".sha256")).read_bytes(),
            "public checksum file mismatch")
    health_path = stage / "public-health.verify"
    download(ORIGIN + "/healthz", health_path)
    health = load_json(health_path)
    require(isinstance(health, dict) and health.get("status") == "ok", "public API health check failed")
    return health


def restore(stage, feed, had_feed, feed_changed, config_changed):
    # Restore the pointer first, before hiding any newly exposed download route.
    if feed_changed:
        if had_feed:
            atomic_copy(stage / "old-android-arm64.json", feed)
        else:
            feed.unlink(missing_ok=True)
            sync_dir(feed.parent)
    if config_changed:
        atomic_copy(stage / "old-nginx.conf", CONFIG)
        reload_nginx()


def publish(stage):
    require(os.geteuid() == 0, "run as root on the deployment host")
    with publication_lock():
        manifest, update, name = validate_stage(stage)
        feed = check_active(manifest, update)
        had_feed = manifest["oldFeedSha256"] is not None
        for directory in (PUBLIC, PUBLIC / "downloads", PUBLIC / "updates"):
            directory.mkdir(mode=0o755, exist_ok=True)
            operator_path(directory, directory=True)
        # Reject different immutable contents before creating a backup or changing routes.
        for filename in (name, name + ".sha256"):
            existing = PUBLIC / "downloads" / filename
            if existing.exists() or existing.is_symlink():
                operator_path(existing)
                require(sha(existing) == sha(stage / filename), f"published version is immutable: {filename}")
        atomic_copy(CONFIG, stage / "old-nginx.conf", mode=0o600)
        if had_feed:
            atomic_copy(feed, stage / "old-android-arm64.json", mode=0o600)
        receipt(stage, "prepared.json", manifest)
        config_changed = feed_changed = False
        try:
            # Recheck the active state immediately before the first public mutation.
            check_active(manifest, update)
            for filename in (name, name + ".sha256"):
                atomic_copy(stage / filename, PUBLIC / "downloads" / filename, immutable=True)
            config_changed = True
            atomic_copy(stage / "nginx.conf", CONFIG)
            reload_nginx()
            await_static_route(update["url"])
            health = verify_public(stage, update, name)
            receipt(stage, "verified-public-apk.json", {"apkSha256": update["sha256"],
                    "size": update["size"], "apiHealth": health})
            feed_changed = True
            atomic_copy(stage / "android-arm64.json", feed)
            public_feed = stage / "public-feed.verify"
            download(FEED_URL, public_feed)
            require(sha(public_feed) == manifest["updateSha256"], "public update feed mismatch")
            result = {"published": True, "version": update["version"], "versionCode": update["versionCode"],
                      "apkSha256": update["sha256"], "updateSha256": manifest["updateSha256"], "apiHealth": health}
            receipt(stage, "published.json", result)
            return result
        except BaseException as error:
            try:
                restore(stage, feed, had_feed, feed_changed, config_changed)
            except BaseException as rollback_error:
                receipt(stage, "rollback-failed.json", {"error": str(error), "rollbackError": str(rollback_error)})
                raise RuntimeError(f"publication failed and rollback needs operator attention: {rollback_error}") from error
            receipt(stage, "rolled-back.json", {"error": str(error), "configRestored": config_changed,
                                                "feedRestored": feed_changed})
            raise


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--stage", type=Path, required=True)
    args = parser.parse_args()
    print(json.dumps(publish(args.stage), ensure_ascii=False))


if __name__ == "__main__":
    main()
