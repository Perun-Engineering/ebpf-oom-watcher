#!/bin/bash

set -e

echo "🔧 Setting up development environment for eBPF OOM Watcher..."

# Check if we're in the right directory
if [ ! -f "Cargo.toml" ]; then
    echo "❌ Error: Please run this script from the project root directory"
    exit 1
fi

# Install pre-commit if not already installed
if ! command -v pre-commit &> /dev/null; then
    echo "📦 Installing pre-commit..."
    pip install pre-commit
fi

# Install pre-commit hooks
echo "🪝 Installing pre-commit hooks..."
pre-commit install

# Install Rust components if not already installed
echo "🦀 Installing required Rust components..."
rustup component add rustfmt clippy

# Install required tools for cross-compilation
echo "🔨 Installing development tools..."
if ! command -v cross &> /dev/null; then
    echo "Installing cross for cross-compilation..."
    cargo install cross --git https://github.com/cross-rs/cross
fi

# Install Helm if not already installed
if ! command -v helm &> /dev/null; then
    echo "⚓ Installing Helm..."
    curl https://raw.githubusercontent.com/helm/helm/main/scripts/get-helm-3 | bash
fi

# Validate Helm chart
echo "📊 Validating Helm chart..."
helm lint helm/oom-watcher

echo ""
echo "✅ Development environment setup complete!"
echo ""
echo "📋 Available commands:"
echo "   - pre-commit run --all-files     # Run all pre-commit hooks"
echo "   - cargo fmt --all               # Format Rust code"
echo "   - cargo clippy --all-targets    # Run Rust linter"
echo "   - helm lint helm/oom-watcher    # Validate Helm chart"
echo "   - ./scripts/build-and-test.sh   # Build and test the project"
echo ""
echo "🎉 Happy coding!"
