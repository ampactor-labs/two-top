#!/usr/bin/env python3
"""Fail on a platform gate that silently hands the browser a desktop path.

`#[cfg(not(target_os = "android"))]` reads like "desktop". It is not: it
means "desktop AND the browser", and the browser build is mostly opened
on a phone. That one habit produced, in a single release, a web build
that installed the KEYBOARD as its only input source on a touchscreen,
asked a phone GPU for a desktop's HDR + bloom chain (measured at 10 fps
on the hardware it ships to), and drew a WASD legend over the arena.

So the gate is not banned — it is made explicit. Write the gate you mean
(`not(any(target_os = "android", target_family = "wasm"))` is usually
it), or, where the browser genuinely belongs on the non-Android side,
say so on the line above:

    // platform-gate-ok: the browser uses the keyboard source too
    #[cfg(not(target_os = "android"))]

A second rule keeps policy out of target gates altogether. In the app
crate, a `target_os` / `target_family` gate may only appear in the
platform adapter (`crates/app/src/platform/`), the bootstrap (`lib.rs`)
and the files that are themselves per-platform I/O backends (listed in
`APP_GATE_ALLOWLIST` with the reason). Everything else is kernel: it asks
`platform::DeviceProfile` at runtime or calls a `platform::` function, so
a policy decided once takes effect in every build.

Run: python3 scripts/check_platform_gates.py
"""

import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
CRATES = ROOT / "crates"
MARKER = "platform-gate-ok"
BARE = re.compile(r'not\s*\(\s*target_os\s*=\s*"android"\s*\)')
# A cfg predicate routinely wraps across lines (rustfmt does it for any
# `all(...)` with two arms), so the whole balanced expression has to be
# read before judging it — `logging.rs` spells the gate correctly and a
# line-at-a-time check calls it a violation.
ATTR = re.compile(r"\bcfg(?:_attr)?\s*\(")

APP_SRC = CRATES / "app" / "src"
TARGET = re.compile(r"\btarget_(?:os|family)\s*=")
# Files outside platform/ that may branch on the target, and why. Each is
# platform I/O, not policy: what it does differs per platform by nature.
APP_GATE_ALLOWLIST = {
    "lib.rs": "bootstrap: the entry points, window/canvas and asset-root setup",
    "logging.rs": "log sinks: stderr, a rolling file, the browser console",
    "paths.rs": "storage backends: the filesystem, Android dirs, localStorage",
    "netplay.rs": "the ICE fetch: a blocking request on a thread, or the page's fetch",
    "share.rs": "the tape post: a blocking request, or the page's fetch and download",
}


def cfg_expressions(text: str):
    """Yield (line_number, full_expression) for every cfg/cfg_attr."""
    for m in ATTR.finditer(text):
        start = m.end() - 1
        depth = 0
        for i in range(start, len(text)):
            if text[i] == "(":
                depth += 1
            elif text[i] == ")":
                depth -= 1
                if depth == 0:
                    yield text.count("\n", 0, m.start()) + 1, text[start : i + 1]
                    break


def main() -> int:
    findings: list[str] = []
    for path in sorted(CRATES.rglob("*.rs")):
        text = path.read_text()
        lines = text.splitlines()
        for n, expr in cfg_expressions(text):
            if not BARE.search(expr):
                continue
            # The correct spellings all name wasm somewhere in the same
            # predicate: not(any(android, wasm)), all(not(a), not(w)), ...
            if "wasm" in expr:
                continue
            # An acknowledgement on the gate's line, or anywhere in the
            # contiguous comment block above it — the reason belongs next
            # to the doc comment explaining the gate, not wedged between
            # it and the attribute.
            context = [lines[n - 1]]
            i = n - 2
            while i >= 0 and lines[i].lstrip().startswith(("//", "#[")):
                context.append(lines[i])
                i -= 1
            if any(MARKER in line for line in context):
                continue
            findings.append(
                f"{path.relative_to(ROOT)}:{n}: bare `not(target_os = \"android\")` "
                f"also matches wasm — name the platform, or mark it "
                f"`// {MARKER}: <why the browser belongs here>`\n    "
                f"{' '.join(expr.split())}"
            )

    for path in sorted(APP_SRC.rglob("*.rs")):
        rel = path.relative_to(APP_SRC)
        if rel.parts[0] == "platform" or str(rel) in APP_GATE_ALLOWLIST:
            continue
        for n, expr in cfg_expressions(path.read_text()):
            if TARGET.search(expr):
                findings.append(
                    f"{path.relative_to(ROOT)}:{n}: a target gate in kernel code — "
                    f"read `platform::DeviceProfile` or call a `platform::` function "
                    f"(or, for genuine per-platform I/O, add the file to "
                    f"APP_GATE_ALLOWLIST with the reason)\n    {' '.join(expr.split())}"
                )

    for f in findings:
        print(f"FAIL {f}")
    if findings:
        print(f"\n{len(findings)} unacknowledged platform gate(s).")
        return 1
    print(
        "platform gates ok — every non-Android gate names wasm or acknowledges it, "
        "and app policy reads DeviceProfile instead of the target."
    )
    return 0


if __name__ == "__main__":
    sys.exit(main())
