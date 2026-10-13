"""Release metadata and publication transaction regressions; no network or root needed."""
from contextlib import ExitStack
import importlib.util
import json
from pathlib import Path
import shutil
import stat
import tempfile
from types import SimpleNamespace
import unittest
from unittest.mock import patch
import zipfile

from android_release import (FEED_URL, ORIGIN, PACKAGE, check_badging, generate_manifest,
                             sha, validate_manifest, write_json)

spec = importlib.util.spec_from_file_location("publish_android", Path(__file__).with_name("publish-android.py"))
publisher = importlib.util.module_from_spec(spec)
spec.loader.exec_module(publisher)


def update(version="0.3.3", code=1006, apk_hash="a" * 64, size=123):
    return {"schemaVersion": 1, "packageName": PACKAGE, "version": version, "versionCode": code,
            "minSdk": 24, "abi": "arm64-v8a", "url": f"{ORIGIN}/downloads/memhot-{version}-arm64.apk",
            "size": size, "sha256": apk_hash, "notes": ["可在设置中检查更新。"],
            "publishedAt": "2026-10-12T12:00:00Z"}


BADGING = """package: name='com.angelkang.memhot' versionCode='1006' versionName='0.3.3'
sdkVersion:'24'
targetSdkVersion:'36'
native-code: 'arm64-v8a'
"""


class MetadataTests(unittest.TestCase):
    def test_manifest_accepts_exact_contract(self):
        self.assertEqual(validate_manifest(update())["versionCode"], 1006)

    def test_manifest_rejects_bad_versions_hashes_urls_and_schema(self):
        mutations = [{"version": "../0.3.3"}, {"version": "00.3.3"}, {"version": "0.3.3-beta"},
                     {"schemaVersion": 2}, {"schemaVersion": True}, {"extra": "unknown"},
                     {"sha256": "A" * 64}, {"sha256": "a" * 63}, {"versionCode": True},
                     {"versionCode": 0}, {"size": True}, {"size": 0}, {"minSdk": 23},
                     {"packageName": "untrusted.app"}, {"abi": "x86_64"}, {"notes": []},
                     {"notes": ["first\nsecond"]}, {"publishedAt": "2026-02-30T12:00:00Z"},
                     {"url": "https://memhot.angelkang.com.evil.invalid/download.apk"},
                     {"url": update()["url"] + "?redirect=yes"}]
        for mutation in mutations:
            with self.subTest(mutation=mutation), self.assertRaises(ValueError):
                validate_manifest(update() | mutation)

    def test_badging_rejects_mismatched_install_metadata(self):
        for before, after in ((PACKAGE, "other.package"), ("1006", "1007"), ("0.3.3", "0.3.2"),
                              ("sdkVersion:'24'", "sdkVersion:'23'"),
                              ("'arm64-v8a'", "'arm64-v8a' 'x86_64'"),
                              ("targetSdkVersion", "application-debuggable\ntargetSdkVersion")):
            with self.subTest(after=after), self.assertRaises(ValueError):
                check_badging(BADGING.replace(before, after), "0.3.3", 1006)

    def test_generated_metadata_comes_from_final_apk_and_matching_notes(self):
        with tempfile.TemporaryDirectory() as temporary:
            apk = Path(temporary) / "final.apk"
            with zipfile.ZipFile(apk, "w") as archive:
                archive.writestr("lib/arm64-v8a/libfixture.so", b"native fixture")
            config = {"version": "0.3.3", "identifier": PACKAGE, "bundle": {"android": {"versionCode": 1006}}}
            notes = {"schemaVersion": 1, "version": "0.3.3", "versionCode": 1006, "notes": ["校验后安装。"]}
            manifest = generate_manifest(apk, config, notes, BADGING, "2026-10-12T12:00:00Z")
            self.assertEqual(manifest["sha256"], sha(apk))
            self.assertEqual(manifest["size"], apk.stat().st_size)
            with self.assertRaisesRegex(ValueError, "version metadata"):
                generate_manifest(apk, config, notes | {"versionCode": 1005}, BADGING)
            with zipfile.ZipFile(apk, "a") as archive:
                archive.writestr("lib/x86_64/libextra.so", b"wrong architecture")
            with self.assertRaisesRegex(ValueError, "ARM64-only"):
                generate_manifest(apk, config, notes, BADGING)

    def test_operator_paths_reject_symlinks_and_writable_or_nonroot_inputs(self):
        for mode, owner in ((stat.S_IFLNK | 0o600, 0), (stat.S_IFREG | 0o666, 0),
                            (stat.S_IFREG | 0o600, 1000), (stat.S_IFDIR | 0o700, 0)):
            path = SimpleNamespace(lstat=lambda: SimpleNamespace(st_mode=mode, st_uid=owner))
            with self.subTest(mode=mode, owner=owner), self.assertRaises(ValueError):
                publisher.operator_path(path)
        path = SimpleNamespace(lstat=lambda: SimpleNamespace(st_mode=stat.S_IFDIR | 0o755, st_uid=0))
        with self.assertRaisesRegex(ValueError, "permissions"):
            publisher.operator_path(path, directory=True, private=True)


class PublicationTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.root = Path(self.temporary.name).resolve()
        self.deployments = self.root / "deployments"
        self.stage = self.deployments / "android-0.3.3"
        self.stage.mkdir(parents=True, mode=0o700)
        self.public = self.root / "public"
        (self.public / "downloads").mkdir(parents=True)
        (self.public / "updates").mkdir()
        self.feed = self.public / "updates/android-arm64.json"
        self.config = self.root / "nginx.conf"
        self.old_config = b"original nginx configuration\n"
        self.config.write_bytes(self.old_config)
        self.name = "memhot-0.3.3-arm64.apk"
        self.apk = self.stage / self.name
        self.apk.write_bytes(b"final signed APK fixture")
        self.update = update(apk_hash=sha(self.apk), size=self.apk.stat().st_size)
        write_json(self.stage / "android-arm64.json", self.update)
        (self.stage / (self.name + ".sha256")).write_text(sha(self.apk) + "  " + self.name + "\n")
        nginx = "\n".join(f"location = {route} {{\n alias /opt/memhot/public{route};\n}}"
                          for route in (f"/downloads/{self.name}", f"/downloads/{self.name}.sha256",
                                        "/updates/android-arm64.json"))
        (self.stage / "nginx.conf").write_text(nginx)
        self.manifest = {"schemaVersion": 1, "version": "0.3.3", "versionCode": 1006,
                         "apkSha256": sha(self.apk), "oldNginxSha256": sha(self.config),
                         "newNginxSha256": sha(self.stage / "nginx.conf"),
                         "updateSha256": sha(self.stage / "android-arm64.json"), "oldFeedSha256": None}
        self.refresh()
        self.events = []
        self.original_copy = publisher.atomic_copy
        self.stack = ExitStack()
        self.addCleanup(self.stack.close)
        self.addCleanup(self.temporary.cleanup)
        for name, value in (("DEPLOYMENTS", self.deployments), ("PUBLIC", self.public), ("CONFIG", self.config)):
            self.stack.enter_context(patch.object(publisher, name, value))
        self.stack.enter_context(patch.object(publisher, "operator_path"))
        self.stack.enter_context(patch.object(publisher.os, "geteuid", return_value=0))
        self.stack.enter_context(patch.object(publisher, "command", side_effect=self.command))
        self.stack.enter_context(patch.object(publisher, "download", side_effect=self.download))
        self.stack.enter_context(patch.object(publisher, "atomic_copy", side_effect=self.copy))

    def refresh(self):
        write_json(self.stage / "manifest.json", self.manifest)

    def command(self, *args):
        self.events.append(tuple(args))
        return "200" if args[:2] == ("curl", "--head") else ""

    def copy(self, source, target, **kwargs):
        self.events.append(("write", target))
        self.original_copy(source, target, **kwargs)

    def download(self, url, destination):
        self.events.append(("download", url))
        if url == ORIGIN + "/healthz":
            write_json(destination, {"status": "ok", "version": "0.3.0"})
        elif url == FEED_URL:
            shutil.copyfile(self.feed, destination)
        else:
            shutil.copyfile(self.public / "downloads" / url.rsplit("/", 1)[1], destination)

    def old_feed(self):
        write_json(self.feed, update("0.3.2", 1005))
        self.manifest["oldFeedSha256"] = sha(self.feed)
        self.refresh()
        return self.feed.read_bytes()

    def test_feed_is_published_only_after_full_apk_checksum_and_health_verification(self):
        result = publisher.publish(self.stage)
        feed_index = self.events.index(("write", self.feed))
        for url in (self.update["url"], self.update["url"] + ".sha256", ORIGIN + "/healthz"):
            self.assertLess(self.events.index(("download", url)), feed_index)
        self.assertTrue((self.stage / "verified-public-apk.json").exists())
        self.assertTrue(result["published"])
        self.assertEqual(json.loads(self.feed.read_text()), self.update)
        with self.assertRaisesRegex(ValueError, "already used"):
            publisher.publish(self.stage)

    def test_nginx_validation_failure_restores_configuration_and_keeps_feed_absent(self):
        def fail_first(*args):
            self.command(*args)
            if self.events.count(("nginx", "-t")) == 1:
                raise RuntimeError("nginx configuration failed")
        with patch.object(publisher, "command", side_effect=fail_first), self.assertRaises(RuntimeError):
            publisher.publish(self.stage)
        self.assertEqual(self.config.read_bytes(), self.old_config)
        self.assertFalse(self.feed.exists())
        self.assertNotIn(("write", self.feed), self.events)
        self.assertEqual(self.events.count(("systemctl", "reload", "nginx")), 1)
        self.assertTrue((self.stage / "rolled-back.json").exists())

    def test_corrupt_public_apk_never_changes_existing_feed_and_rolls_back_nginx(self):
        previous = self.old_feed()
        with patch.object(publisher, "download", side_effect=lambda _, path: path.write_bytes(b"corrupt")), \
                self.assertRaisesRegex(ValueError, "public APK"):
            publisher.publish(self.stage)
        self.assertEqual(self.feed.read_bytes(), previous)
        self.assertEqual(self.config.read_bytes(), self.old_config)
        self.assertNotIn(("write", self.feed), self.events)

    def test_failure_after_feed_switch_restores_feed_before_configuration(self):
        previous = self.old_feed()
        def fail_feed(url, destination):
            if url == FEED_URL:
                raise RuntimeError("public feed failed")
            self.download(url, destination)
        with patch.object(publisher, "download", side_effect=fail_feed), self.assertRaises(RuntimeError):
            publisher.publish(self.stage)
        self.assertEqual(self.feed.read_bytes(), previous)
        self.assertEqual(self.config.read_bytes(), self.old_config)
        feed_writes = [index for index, event in enumerate(self.events) if event == ("write", self.feed)]
        config_writes = [index for index, event in enumerate(self.events) if event == ("write", self.config)]
        self.assertEqual(len(feed_writes), 2)
        self.assertLess(feed_writes[-1], config_writes[-1])

    def test_first_feed_is_removed_when_its_public_verification_fails(self):
        def fail_feed(url, destination):
            if url == FEED_URL:
                destination.write_bytes(b"stale edge response")
            else:
                self.download(url, destination)
        with patch.object(publisher, "download", side_effect=fail_feed), \
                self.assertRaisesRegex(ValueError, "public update feed"):
            publisher.publish(self.stage)
        self.assertFalse(self.feed.exists())
        self.assertEqual(self.config.read_bytes(), self.old_config)

    def test_changed_active_configuration_or_feed_is_rejected_before_mutation(self):
        self.old_feed()
        for target in (self.config, self.feed):
            previous = target.read_bytes()
            target.write_bytes(b"concurrent operator edit")
            with self.subTest(target=target), self.assertRaisesRegex(ValueError, "changed"):
                publisher.publish(self.stage)
            self.assertFalse((self.stage / "old-nginx.conf").exists())
            target.write_bytes(previous)

    def test_stage_version_or_hash_mismatch_is_rejected(self):
        for field, bad in (("versionCode", 1007), ("apkSha256", "b" * 64),
                           ("updateSha256", "c" * 64), ("schemaVersion", True)):
            good = self.manifest[field]
            self.manifest[field] = bad
            self.refresh()
            with self.subTest(field=field), self.assertRaises(ValueError):
                publisher.publish(self.stage)
            self.assertFalse((self.stage / "old-nginx.conf").exists())
            self.manifest[field] = good

    def test_existing_version_cannot_be_overwritten(self):
        target = self.public / "downloads" / self.name
        target.write_bytes(b"different immutable version")
        with self.assertRaisesRegex(ValueError, "immutable"):
            publisher.publish(self.stage)
        self.assertEqual(target.read_bytes(), b"different immutable version")
        self.assertEqual(self.config.read_bytes(), self.old_config)
        self.assertFalse((self.stage / "old-nginx.conf").exists())

    def test_existing_identical_version_can_be_reused_after_an_earlier_rollback(self):
        shutil.copyfile(self.apk, self.public / "downloads" / self.name)
        self.assertTrue(publisher.publish(self.stage)["published"])

    def test_feed_downgrade_and_nonroot_operator_are_rejected(self):
        write_json(self.feed, update("0.3.4", 1007))
        self.manifest["oldFeedSha256"] = sha(self.feed)
        self.refresh()
        with self.assertRaisesRegex(ValueError, "increase versionCode"):
            publisher.publish(self.stage)
        with patch.object(publisher.os, "geteuid", return_value=1000), self.assertRaisesRegex(ValueError, "root"):
            publisher.publish(self.stage)


class HttpTests(unittest.TestCase):
    def test_route_waits_for_new_workers_without_retrying_publication(self):
        with patch.object(publisher, "command", side_effect=["404", "200"]) as command, \
                patch.object(publisher.time, "sleep") as sleep:
            publisher.await_static_route(update()["url"])
        self.assertEqual(command.call_count, 2)
        sleep.assert_called_once_with(1)
        self.assertNotIn("--location", command.call_args.args)

    def test_route_wait_is_bounded_and_does_not_accept_redirects(self):
        with patch.object(publisher, "command", return_value="404") as command, \
                patch.object(publisher.time, "sleep") as sleep, \
                self.assertRaisesRegex(ValueError, "did not become ready"):
            publisher.await_static_route(update()["url"])
        self.assertEqual(command.call_count, 6)
        self.assertEqual(sleep.call_count, 5)
        with patch.object(publisher, "command", return_value="302") as command, \
                self.assertRaisesRegex(ValueError, "unexpected static route"):
            publisher.await_static_route(update()["url"])
        self.assertEqual(command.call_count, 1)

    def test_redirect_is_rejected_and_curl_does_not_follow_redirects(self):
        with tempfile.TemporaryDirectory() as temporary:
            path = Path(temporary) / "download"
            path.write_bytes(b"redirect")
            with patch.object(publisher, "command", return_value="302") as command, \
                    self.assertRaisesRegex(ValueError, "direct HTTPS 200"):
                publisher.download(FEED_URL, path)
            args = command.call_args.args
            self.assertNotIn("--location", args)
            self.assertEqual(args[args.index("--max-redirs") + 1], "0")
            self.assertEqual(args[args.index("--proto") + 1], "=https")


if __name__ == "__main__":
    unittest.main()
