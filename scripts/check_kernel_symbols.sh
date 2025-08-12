#!/bin/bash

echo "=== Checking OOM-related kernel symbols ==="
echo

echo "1. Checking if oom_kill_process is available in /proc/kallsyms:"
if grep -q "oom_kill_process" /proc/kallsyms 2>/dev/null; then
    grep "oom_kill_process" /proc/kallsyms
    echo "✅ oom_kill_process found in kernel symbols"
else
    echo "❌ oom_kill_process NOT found in kernel symbols"
fi
echo

echo "2. Other OOM-related symbols in kernel:"
grep -i "oom" /proc/kallsyms 2>/dev/null | head -10
echo

echo "3. Checking available kprobe events:"
if [ -r /sys/kernel/debug/tracing/available_filter_functions ]; then
    echo "Searching for oom functions in available_filter_functions:"
    grep -i "oom" /sys/kernel/debug/tracing/available_filter_functions 2>/dev/null | head -10
else
    echo "❌ Cannot read /sys/kernel/debug/tracing/available_filter_functions"
    echo "   This might require root privileges or debugfs to be mounted"
fi
echo

echo "4. Kernel version:"
uname -r
echo

echo "5. Checking if kprobes are enabled:"
if [ -d /sys/kernel/debug/tracing ]; then
    echo "✅ Tracing debugfs is available"
    if [ -f /sys/kernel/debug/tracing/kprobe_events ]; then
        echo "✅ kprobe_events file exists"
    else
        echo "❌ kprobe_events file not found"
    fi
else
    echo "❌ Tracing debugfs not available"
fi
echo

echo "6. Current kprobe events:"
if [ -r /sys/kernel/debug/tracing/kprobe_events ]; then
    cat /sys/kernel/debug/tracing/kprobe_events 2>/dev/null || echo "No kprobe events currently active"
else
    echo "❌ Cannot read kprobe_events (may need root privileges)"
fi 