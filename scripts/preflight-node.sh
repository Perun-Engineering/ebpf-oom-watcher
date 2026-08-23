#!/bin/bash
#
# Pre-flight check for a node before deploying the OOM watcher.
#
# Run this on the node itself, or inside a pod that mounts the host's /sys:
#
#   kubectl debug node/<node> -it --image=busybox -- sh -c 'chroot /host' < scripts/preflight-node.sh
#
# It asserts the same thing the binary asserts at startup (see oom-watcher/src/tracepoint.rs)
# so a node can be ruled in or out without deploying anything.

set -uo pipefail

# The layout MarkVictimArgs decodes, as "name offset size".
readonly REQUIRED_FIELDS=(
    "pid 8 4"
    "comm 12 4"
    "total_vm 16 8"
    "anon_rss 24 8"
    "file_rss 32 8"
    "shmem_rss 40 8"
    "uid 48 4"
    "pgtables 56 8"
    "oom_score_adj 64 2"
)

failures=0
fail() { echo "❌ $1"; failures=$((failures + 1)); }
pass() { echo "✅ $1"; }

echo "=== oom-watcher pre-flight ==="
echo

echo "1. Kernel"
echo "   $(uname -srm)"
kernel_release=$(uname -r)
kernel_major=${kernel_release%%.*}
rest=${kernel_release#*.}
kernel_minor=${rest%%.*}
if [ "$kernel_major" -gt 6 ] 2>/dev/null || { [ "$kernel_major" -eq 6 ] && [ "$kernel_minor" -ge 9 ]; } 2>/dev/null; then
    pass "kernel $kernel_release is 6.9+ (extended oom:mark_victim tracepoint)"
else
    fail "kernel $kernel_release is older than 6.9 — the extended oom:mark_victim fields landed in 6.9"
fi
echo

echo "2. Locating the tracepoint format file"
# FORMAT_FILE overrides the search, so a format file captured from another node can be
# checked from anywhere.
format_file="${FORMAT_FILE:-}"
if [ -z "$format_file" ]; then
    for candidate in \
        /sys/kernel/tracing/events/oom/mark_victim/format \
        /sys/kernel/debug/tracing/events/oom/mark_victim/format; do
        if [ -r "$candidate" ]; then
            format_file="$candidate"
            break
        fi
    done
fi

if [ -z "$format_file" ]; then
    fail "cannot read oom:mark_victim format from tracefs or debugfs (need root, and tracefs mounted)"
    echo
    echo "=== 1 or more checks failed — do not deploy to this node ==="
    exit 1
fi
pass "found $format_file"
echo

echo "3. Tracepoint layout"
# Lines look like: 	field:unsigned long total_vm;	offset:16;	size:8;	signed:0;
for spec in "${REQUIRED_FIELDS[@]}"; do
    read -r name want_offset want_size <<<"$spec"
    line=$(grep -E "field:[^;]*[ *]${name};" "$format_file" | head -1)
    if [ -z "$line" ]; then
        fail "field '${name}' is missing"
        continue
    fi
    offset=$(echo "$line" | sed -n 's/.*offset:\([0-9]*\).*/\1/p')
    size=$(echo "$line" | sed -n 's/.*size:\([0-9]*\).*/\1/p')
    if [ "$offset" = "$want_offset" ] && [ "$size" = "$want_size" ]; then
        pass "${name}: offset ${offset} size ${size}"
    else
        fail "${name}: expected offset ${want_offset} size ${want_size}, found offset ${offset} size ${size}"
    fi
done
echo

echo "4. Page size (BPF ring buffer must be a power-of-2 multiple of it)"
page_size=$(getconf PAGE_SIZE 2>/dev/null || echo unknown)
echo "   PAGE_SIZE=${page_size}"
if [ "$page_size" != "unknown" ] && [ "$page_size" -le 262144 ] 2>/dev/null; then
    pass "the probe's 256 KiB EVENTS buffer is a valid size here"
else
    fail "PAGE_SIZE ${page_size} exceeds the probe's 256 KiB ring buffer — map creation would fail"
fi
echo

echo "5. Container runtime (which cgroup layout resolution will see)"
if [ -r /proc/1/cgroup ]; then
    echo "   /proc/1/cgroup: $(head -1 /proc/1/cgroup)"
    if grep -qE 'cri-containerd|crio|docker|kubepods' /proc/1/cgroup 2>/dev/null; then
        pass "recognised cgroup layout"
    else
        echo "ℹ️  host process is outside a container — check a workload pod's PID instead:"
        echo "   cat /proc/\$(pgrep -f <app> | head -1)/cgroup"
    fi
else
    fail "cannot read /proc/1/cgroup"
fi
echo

if [ "$failures" -eq 0 ]; then
    echo "=== all checks passed — this node can run the OOM watcher ==="
    exit 0
fi
echo "=== ${failures} check(s) failed — the watcher will refuse to start on this node ==="
exit 1
