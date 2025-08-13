#!/bin/bash

set -e

echo "🔨 Building eBPF OOM Watcher with Kubernetes integration..."

# Check if we're in the right directory
if [ ! -f "Cargo.toml" ]; then
    echo "❌ Error: Please run this script from the project root directory"
    exit 1
fi

# Build the project
echo "📦 Building the project..."
cargo build --release

echo "🧪 Running basic checks..."

# Check if the binary was built
if [ ! -f "target/release/oom-watcher" ]; then
    echo "❌ Error: Binary not found at target/release/oom-watcher"
    exit 1
fi

echo "✅ Build completed successfully!"

# Test if the binary can start (will fail without root, but we can check for basic errors)
echo "🔍 Testing binary startup (will fail without root privileges - this is expected)..."
timeout 5s ./target/release/oom-watcher || true

echo "📋 Available next steps:"
echo "   1. Build Docker image: docker build -f Dockerfile.production -t oom-watcher:latest ."
echo "   2. Deploy to Kubernetes: kubectl apply -f k8s/daemonset.yaml"
echo "   3. Test locally with root: sudo ./target/release/oom-watcher"
echo "   4. Trigger test OOM: python3 scripts/trigger_oom.py"

echo ""
echo "🎉 Build and basic validation completed!"