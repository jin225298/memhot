#!/usr/bin/env python3
"""Regenerate ignored Tauri Android scaffolding and apply reviewed app settings."""
import re
import shutil
import subprocess
from pathlib import Path


def main():
    root = Path(__file__).resolve().parents[2]
    app = root / "apps" / "memhot"
    android = app / "src-tauri" / "gen" / "android"
    manifest = android / "app" / "src" / "main" / "AndroidManifest.xml"
    if not manifest.is_file():
        subprocess.run(
            ["npm", "exec", "--", "tauri", "android", "init", "--ci", "--skip-targets-install"],
            cwd=app,
            check=True,
        )
    build = android / "app" / "build.gradle.kts"
    text = build.read_text()
    text = re.sub(r"compileSdk\s*=\s*\d+", "compileSdk = 36", text)
    text = re.sub(r"targetSdk\s*=\s*\d+", "targetSdk = 36", text)
    ndk = 'ndkVersion = "29.0.14206865"'
    if re.search(r'ndkVersion\s*=\s*"[^"]*"', text):
        text = re.sub(r'ndkVersion\s*=\s*"[^"]*"', ndk, text)
    else:
        text = text.replace("android {", "android {\n    " + ndk, 1)
    if "testInstrumentationRunner =" not in text:
        text = text.replace(
            "defaultConfig {",
            'defaultConfig {\n        testInstrumentationRunner = "androidx.test.runner.AndroidJUnitRunner"',
            1,
        )
    if "memhotTestBuildType" not in text:
        text = text.replace("android {", 'android {\n    testBuildType = providers.gradleProperty("memhotTestBuildType").orElse("debug").get()', 1)
    text = text.replace('testInstrumentationRunner = "androidx.test.runner.AndroidJUnitRunner"',
        'testInstrumentationRunner = providers.gradleProperty("memhotTestRunner").orElse("androidx.test.runner.AndroidJUnitRunner").get()')
    text = re.sub(
        r'androidTestImplementation\("androidx.test.ext:junit:[^"]+"\)',
        'androidTestImplementation("androidx.test.ext:junit:1.2.1")',
        text,
    )
    if 'androidx.test:runner:' not in text:
        text = text.replace(
            "dependencies {",
            'dependencies {\n    androidTestImplementation("androidx.test:runner:1.6.2")',
            1,
        )
    build.write_text(text)
    text = manifest.read_text()
    if "android:allowBackup=" in text:
        text = re.sub(r'android:allowBackup="[^"]*"', 'android:allowBackup="false"', text)
    else:
        text = text.replace("<application", '<application android:allowBackup="false"', 1)
    manifest.write_text(text)
    activity = root / "ops" / "memhot" / "android" / "MainActivity.kt"
    target = android / "app" / "src" / "main" / "java" / "com" / "angelkang" / "memhot" / activity.name
    target.parent.mkdir(parents=True, exist_ok=True)
    shutil.copyfile(activity, target)
    for smoke in (root / "ops" / "memhot" / "android-test").iterdir():
        if smoke.suffix not in {".kt", ".java"}:
            continue
        target = android / "app" / "src" / "androidTest" / "java" / "com" / "angelkang" / "memhot"
        target.mkdir(parents=True, exist_ok=True)
        shutil.copyfile(smoke, target / smoke.name)
    print("Android prepared: API36, NDK29.0.14206865, private automatic backup disabled")


if __name__ == "__main__":
    main()
