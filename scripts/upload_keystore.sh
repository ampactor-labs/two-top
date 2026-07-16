#!/usr/bin/env bash
# One-shot: put the dev box's debug keystore into the repo's APK_KEYSTORE
# secret so CI signs releases with the same key as phone.sh sideloads —
# Android then installs the public APK over them as a plain update.
# Run me from anywhere in the repo: bash scripts/upload_keystore.sh
set -euo pipefail

KS="$HOME/.android/debug.keystore"
[ -f "$KS" ] || { echo "no keystore at $KS"; exit 1; }

base64 -w0 "$KS" | gh secret set APK_KEYSTORE --repo ampactor-labs/two-top
echo "APK_KEYSTORE set:"
gh secret list --repo ampactor-labs/two-top | grep APK_KEYSTORE
echo "Next: rerun the APK workflow (or push) and the release re-signs with this key."
