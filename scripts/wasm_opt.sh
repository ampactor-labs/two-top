#!/usr/bin/env bash
# Size-optimize a bindgen'd wasm module in place.
#
# One script, so the deploy (web.yml) and the browser determinism lane
# (wasm.yml) run the BYTE-IDENTICAL transformation. That matters more than
# the tidiness: wasm-opt rewrites the module after rustc and bindgen are
# done with it, and a rewrite that broke the game would otherwise only be
# discovered by whoever opened the site. Running the same pass under the
# 1800-frame headless checksum probe is what makes it safe to ship.
#
# Usage: scripts/wasm_opt.sh <path/to/app_bg.wasm>
set -euo pipefail

target="${1:?usage: wasm_opt.sh <module.wasm>}"

# Bevy's output uses post-MVP features; without naming them wasm-opt
# validates against the 2017 baseline and refuses the module outright.
FEATURES=(
  --enable-bulk-memory
  --enable-reference-types
  --enable-mutable-globals
  --enable-nontrapping-float-to-int
  --enable-sign-ext
)

before=$(stat -c%s "$target")
wasm-opt -Oz --strip-debug --strip-producers "${FEATURES[@]}" \
  -o "${target}.opt" "$target"
mv "${target}.opt" "$target"
after=$(stat -c%s "$target")

# Both numbers, because they say different things and only one of them is
# the point. Measured on the v0.15 probe build: 28.5 MB -> 22.1 MB raw,
# but only 6.94 MB -> 6.78 MB gzipped. Pages serves the gzipped bytes, so
# the DOWNLOAD barely moves (~2%). The raw size is the payoff: that is
# what the browser decompresses, parses and holds, and iOS Safari kills
# tabs over exactly that. 6.4 MB less peak memory on an iPhone is worth
# more here than 160 KB less transfer.
gz=$(gzip -9 -c "$target" | wc -c)
echo "wasm-opt: ${before} -> ${after} bytes raw (${gz} gzipped, which is what Pages serves)"
