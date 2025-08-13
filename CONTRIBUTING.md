# Contributing to eBPF OOM Watcher

Thank you for your interest in contributing to the eBPF OOM Watcher project!

## Development Setup

1. **Clone the repository:**
   ```bash
   git clone https://github.com/Perun-Engineering/ebpf-oom-watcher.git
   cd ebpf-oom-watcher
   ```

2. **Set up development environment:**
   ```bash
   ./scripts/setup-dev.sh
   ```

3. **Install development dependencies:**
   ```bash
   # Install Rust (if not already installed)
   curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh

   # Install eBPF toolchain
   rustup component add rust-src --toolchain nightly
   cargo install bpf-linker
   ```

## Development Workflow

### Semantic Commit Messages

We use conventional commit messages. Your commits should follow this format:

```
<type>[optional scope]: <description>

[optional body]

[optional footer(s)]
```

**Types:**
- `feat`: New feature
- `fix`: Bug fix
- `docs`: Documentation changes
- `style`: Code style changes (formatting, etc.)
- `refactor`: Code refactoring
- `perf`: Performance improvements
- `test`: Adding or updating tests
- `build`: Build system changes
- `ci`: CI/CD changes
- `chore`: Maintenance tasks

**Scopes (optional):**
- `core`: Core eBPF functionality
- `k8s`: Kubernetes integration
- `metrics`: Prometheus metrics
- `docker`: Container/Docker related
- `helm`: Helm chart
- `ci`: CI/CD workflows

**Examples:**
```
feat(k8s): add pod metadata enrichment
fix(metrics): correct memory usage calculation
docs: update deployment instructions
```

### Pull Request Process

1. **Create a feature branch:**
   ```bash
   git checkout -b feat/your-feature-name
   ```

2. **Make your changes and commit them:**
   ```bash
   git add .
   git commit -m "feat: your feature description"
   ```

3. **Run local checks:**
   ```bash
   # Run pre-commit hooks
   pre-commit run --all-files

   # Build and test
   cargo test
   cargo clippy --all-targets --all-features -- -D warnings
   cargo fmt --all -- --check

   # Test Helm chart
   helm lint helm/oom-watcher
   ```

4. **Push and create PR:**
   ```bash
   git push origin feat/your-feature-name
   ```

5. **Create a Pull Request** with:
   - Descriptive title following semantic convention
   - Clear description of changes
   - Reference to any related issues

### Branch Strategy

- `main`: Production-ready code
- `develop`: Integration branch (if needed)
- `feat/*`: Feature branches
- `fix/*`: Bug fix branches
- `release/*`: Release preparation branches

### Code Quality Standards

- **Rust Code:**
  - Follow standard Rust formatting (`cargo fmt`)
  - Pass all clippy lints (`cargo clippy`)
  - Include tests for new functionality
  - Document public APIs

- **Kubernetes/Helm:**
  - Follow Kubernetes best practices
  - Validate YAML with yamllint
  - Test Helm chart with `helm lint`

- **Documentation:**
  - Update README.md for significant changes
  - Document configuration options
  - Include examples for new features

## Testing

### Local Testing

```bash
# Run Rust tests
cargo test

# Build for different architectures
cross build --target x86_64-unknown-linux-gnu --release
cross build --target aarch64-unknown-linux-gnu --release

# Test eBPF program (requires root)
sudo ./target/release/oom-watcher

# Test Helm chart
helm template helm/oom-watcher | kubectl apply --dry-run=client -f -
```

### CI/CD Pipeline

Our CI/CD pipeline runs on every PR and includes:

1. **Linting:** Code formatting and style checks
2. **Testing:** Unit tests and integration tests
3. **Building:** Multi-architecture builds
4. **Security:** Dependency scanning
5. **Helm:** Chart validation

## Release Process

Releases are automated using semantic-release:

1. **Merge to main** triggers the release workflow
2. **Semantic versioning** based on commit messages
3. **Multi-arch container images** pushed to GHCR
4. **Helm chart** published to OCI registry
5. **Draft release** created for manual review

## Security

- Report security vulnerabilities privately
- Follow security best practices
- Run with minimal required privileges
- Regular dependency updates

## Getting Help

- **Issues:** Use GitHub issues for bugs and feature requests
- **Discussions:** Use GitHub discussions for questions
- **Documentation:** Check the README and k8s/README.md

## Code of Conduct

- Be respectful and inclusive
- Focus on constructive feedback
- Help others learn and grow
- Follow project guidelines

Thank you for contributing! 🚀
