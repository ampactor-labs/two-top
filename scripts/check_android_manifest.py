#!/usr/bin/env python3
"""Ship gates for the Android build's manifest metadata.

`apk.yml` only runs on a push to `main`, and nothing else in the tree
reads `[package.metadata.android]` at all — so a manifest mistake was
invisible until it had already broken the published release. That is
exactly what happened: cargo-apk 0.10.0 panics at config-parse time if
`version_code`/`version_name` appear in the TOML (it derives both from
the crate version), and the failure surfaced only as a red APK job after
the merge.

These checks are the cheap half of that workflow — parse-time rules and
the two ship blockers that can silently ride a green build — so they run
on every branch and every PR, in a second, with no NDK.

Run: python3 scripts/check_android_manifest.py
"""

import sys
import tomllib
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
APP = ROOT / "crates" / "app" / "Cargo.toml"
WORKSPACE = ROOT / "Cargo.toml"
APK_WORKFLOW = ROOT / ".github" / "workflows" / "apk.yml"

# cargo-apk derives these from the crate version and panics outright if it
# finds either in the table ("version_name should not be set in TOML",
# cargo-apk 0.10.0 apk.rs:83).
DERIVED_KEYS = ("version_code", "version_name")

failures: list[str] = []


def fail(msg: str) -> None:
    failures.append(msg)


def main() -> int:
    # A duplicate `[package.metadata.android]` table is a TOML parse error,
    # so simply loading the file is itself one of the gates.
    try:
        app = tomllib.loads(APP.read_text())
    except tomllib.TOMLDecodeError as e:
        print(f"FAIL {APP.relative_to(ROOT)} does not parse as TOML: {e}")
        return 1
    workspace = tomllib.loads(WORKSPACE.read_text())

    android = app.get("package", {}).get("metadata", {}).get("android")
    if android is None:
        fail("crates/app/Cargo.toml has no [package.metadata.android] table")
        android = {}

    for key in DERIVED_KEYS:
        if key in android:
            fail(
                f"[package.metadata.android] sets `{key}`; cargo-apk derives it "
                "from the crate version and panics when it is present. Bump "
                "[workspace.package] version instead."
            )

    # The version cargo-apk turns into versionName/versionCode. Android's
    # downgrade protection is keyed on versionCode and never engages while
    # every build ships the placeholder.
    version = workspace.get("workspace", {}).get("package", {}).get("version")
    if version is None:
        fail("[workspace.package] has no `version` for cargo-apk to derive from")
    elif version == "0.0.0":
        fail(
            "[workspace.package] version is still 0.0.0 — every APK would carry "
            "the same versionCode and Android would treat an older build as a "
            "same-version reinstall"
        )
    if app.get("package", {}).get("version") != {"workspace": True}:
        fail("crates/app must take `version.workspace = true`, not its own version")

    # A debuggable public build hands `adb shell run-as` the private data
    # dir — including the ed25519 seed the signed-results pillar rests on.
    application = android.get("application", {})
    if application.get("debuggable") is not False:
        fail(
            "[package.metadata.android.application] debuggable must be `false` in "
            "committed code (flip it locally, never commit it)"
        )

    # The launcher icon: without `icon` (and the res dir holding it) the
    # phone shows the system's generic robot for the app.
    icon = application.get("icon")
    if icon != "@mipmap/ic_launcher":
        fail(
            "[package.metadata.android.application] icon must be "
            '"@mipmap/ic_launcher" (scripts/generate_android_icons.py)'
        )
    res = android.get("resources")
    res_dir = APP.parent / res if res else None
    if res_dir is None or not (res_dir / "mipmap-anydpi-v26" / "ic_launcher.xml").exists():
        fail(
            "[package.metadata.android] resources must point at the res dir "
            "holding the launcher icon (run scripts/generate_android_icons.py)"
        )

    # TURN credentials are compile-time-baked and trivially extractable from
    # any distributed binary, so they must never enter the public APK build.
    if APK_WORKFLOW.exists():
        for n, line in enumerate(APK_WORKFLOW.read_text().splitlines(), 1):
            stripped = line.lstrip()
            if stripped.startswith("#"):
                continue
            if "TWOTOP_TURN_" in stripped:
                fail(
                    f".github/workflows/apk.yml:{n} bakes a TWOTOP_TURN_* value "
                    "into the public APK"
                )

    for msg in failures:
        print(f"FAIL {msg}")
    if failures:
        return 1
    print(f"android manifest gates ok (version {version}, debuggable false)")
    return 0


if __name__ == "__main__":
    sys.exit(main())
