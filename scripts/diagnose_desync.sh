#!/usr/bin/env bash
# diagnose_desync.sh — find the first divergent (frame, column) between two
# replay_sync checksum TSVs and print both rows for inspection.
#
# Usage:
#   diagnose_desync.sh <tsv_a> <tsv_b> [<demo.bmrg>]
#
# If <demo.bmrg> is supplied, the script additionally invokes
# `replay_sync --dump-state-at <frame>` on the divergent frame so the
# operator sees the full sim state on each side. Without it, the script
# limits itself to TSV-level diagnosis.
#
# Exit codes:
#   0 — files identical (no divergence)
#   1 — divergence found (details printed)
#   2 — usage error or missing files
#
# Phase 5 of BUILD_PLAN: this is the operator-facing diff tool that turns a
# red CI run on the determinism matrix into an actionable report.

set -euo pipefail

if [[ $# -lt 2 || $# -gt 3 ]]; then
    echo "usage: $0 <tsv_a> <tsv_b> [<demo.bmrg>]" >&2
    exit 2
fi

A="$1"
B="$2"
DEMO="${3:-}"

if [[ ! -f "$A" ]]; then
    echo "diagnose_desync: missing $A" >&2
    exit 2
fi
if [[ ! -f "$B" ]]; then
    echo "diagnose_desync: missing $B" >&2
    exit 2
fi

if cmp -s "$A" "$B"; then
    echo "diagnose_desync: $A and $B are identical"
    exit 0
fi

# Read header (line 1 of either file — they must agree on schema).
HEADER_A="$(head -n1 "$A")"
HEADER_B="$(head -n1 "$B")"
if [[ "$HEADER_A" != "$HEADER_B" ]]; then
    echo "diagnose_desync: TSV headers differ" >&2
    echo "  A: $HEADER_A" >&2
    echo "  B: $HEADER_B" >&2
    exit 1
fi

# Tab-split the header into a column-name array (1-indexed for awk-friendly
# arithmetic — we'll reach into ${COL_NAMES[i-1]} below).
IFS=$'\t' read -r -a COL_NAMES <<<"$HEADER_A"

# Walk rows until the first non-matching pair. paste joins with a tab so the
# A and B halves are split by column count.
NCOL="${#COL_NAMES[@]}"

# Skip the header row (NR>1); for the first row whose A-side != B-side,
# print frame and the first differing column index, then exit.
awk -v ncol="$NCOL" -v fa="$A" -v fb="$B" '
BEGIN {
    OFS = "\t"
}
{
    getline lineB < fb
    if (NR == 1) next
    if ($0 == lineB) next

    n_a = split($0, a, "\t")
    n_b = split(lineB, b, "\t")
    if (n_a != n_b) {
        printf("DIVERGE_LINE_COUNT\t%d\t%d\t%d\n", NR, n_a, n_b)
        exit
    }
    for (i = 1; i <= n_a; i++) {
        if (a[i] != b[i]) {
            printf("DIVERGE\t%s\t%d\t%s\t%s\n", a[1], i, a[i], b[i])
            exit
        }
    }
}
' "$A" >/tmp/diagnose_desync_$$.out

if [[ ! -s /tmp/diagnose_desync_$$.out ]]; then
    echo "diagnose_desync: cmp says different, awk found no row diff — \
TSVs may differ only in trailing whitespace" >&2
    rm -f /tmp/diagnose_desync_$$.out
    exit 1
fi

LINE="$(cat /tmp/diagnose_desync_$$.out)"
rm -f /tmp/diagnose_desync_$$.out
KIND="$(printf '%s' "$LINE" | cut -f1)"

case "$KIND" in
    DIVERGE)
        FRAME="$(printf '%s' "$LINE" | cut -f2)"
        COL_IDX="$(printf '%s' "$LINE" | cut -f3)"
        VAL_A="$(printf '%s' "$LINE" | cut -f4)"
        VAL_B="$(printf '%s' "$LINE" | cut -f5)"
        COL_NAME="${COL_NAMES[COL_IDX-1]}"
        echo "diagnose_desync: divergence at frame $FRAME, column $COL_NAME (col index $COL_IDX)"
        echo "  $A: $VAL_A"
        echo "  $B: $VAL_B"

        # Print the surrounding rows from each file for context.
        echo "--- $A (frame $FRAME ±2) ---"
        awk -v target="$FRAME" 'NR==1 || ($1 >= target-2 && $1 <= target+2)' "$A"
        echo "--- $B (frame $FRAME ±2) ---"
        awk -v target="$FRAME" 'NR==1 || ($1 >= target-2 && $1 <= target+2)' "$B"

        if [[ -n "$DEMO" && -x "$(command -v cargo)" ]]; then
            echo "--- replay_sync --dump-state-at $FRAME ($DEMO) ---"
            cargo run --quiet -p replay_sync --bin replay_sync -- \
                --demo "$DEMO" --dump-state-at "$FRAME" || true
        fi
        exit 1
        ;;
    DIVERGE_LINE_COUNT)
        ROW="$(printf '%s' "$LINE" | cut -f2)"
        N_A="$(printf '%s' "$LINE" | cut -f3)"
        N_B="$(printf '%s' "$LINE" | cut -f4)"
        echo "diagnose_desync: row $ROW has $N_A columns in $A vs $N_B in $B"
        exit 1
        ;;
    *)
        echo "diagnose_desync: unrecognized awk output: $LINE" >&2
        exit 1
        ;;
esac
