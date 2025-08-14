## [1.0.7](https://github.com/Perun-Engineering/ebpf-oom-watcher/compare/v1.0.6...v1.0.7) (2025-08-14)


### Bug Fixes

* correct eBPF dummy source file in Dockerfile optimization ([4915c62](https://github.com/Perun-Engineering/ebpf-oom-watcher/commit/4915c6287784ee2ac3f9238f0558de0bafb28d44))
* optimize Docker build with minimal approach and fix eBPF path issue ([18f5d8a](https://github.com/Perun-Engineering/ebpf-oom-watcher/commit/18f5d8ae0158ab205cc7d12ca27faffded97d81c))

## [1.0.6](https://github.com/Perun-Engineering/ebpf-oom-watcher/compare/v1.0.5...v1.0.6) (2025-08-14)


### Bug Fixes

* optimize Docker build with dependency caching and reduced scope ([dcad329](https://github.com/Perun-Engineering/ebpf-oom-watcher/commit/dcad3299f792922ca02ec6a0ad8bd1cf42011a38))

## [1.0.5](https://github.com/Perun-Engineering/ebpf-oom-watcher/compare/v1.0.4...v1.0.5) (2025-08-14)


### Bug Fixes

* temporarily build only AMD64 images to avoid ARM64 timeout issues ([d5137f3](https://github.com/Perun-Engineering/ebpf-oom-watcher/commit/d5137f395d061615e6056181dc96b913b5ed7d65))

## [1.0.4](https://github.com/Perun-Engineering/ebpf-oom-watcher/compare/v1.0.3...v1.0.4) (2025-08-13)


### Bug Fixes

* use nightly rust image and ensure rust-src component is available ([7ea6d5f](https://github.com/Perun-Engineering/ebpf-oom-watcher/commit/7ea6d5f16ffe138ca185bb95362b0c11b30964c5))

## [1.0.3](https://github.com/Perun-Engineering/ebpf-oom-watcher/compare/v1.0.2...v1.0.3) (2025-08-13)


### Bug Fixes

* use --locked flag for bpf-linker and update to Rust 1.82 ([2ca2bda](https://github.com/Perun-Engineering/ebpf-oom-watcher/commit/2ca2bda0fd31d7fbdeddd7d9672a6cb16e247795))

## [1.0.2](https://github.com/Perun-Engineering/ebpf-oom-watcher/compare/v1.0.1...v1.0.2) (2025-08-13)


### Bug Fixes

* update Rust version to 1.78 for bpf-linker compatibility ([ae49ede](https://github.com/Perun-Engineering/ebpf-oom-watcher/commit/ae49edeae24dabbd1a2ea7eeec823b8cf40127f0))

## [1.0.1](https://github.com/Perun-Engineering/ebpf-oom-watcher/compare/v1.0.0...v1.0.1) (2025-08-13)


### Bug Fixes

* properly set GitHub Actions outputs for release workflow jobs ([94ae6c4](https://github.com/Perun-Engineering/ebpf-oom-watcher/commit/94ae6c4779c4d9598b3263e81ef8a7126667a906))

# 1.0.0 (2025-08-13)


### Bug Fixes

* add Cross.toml to pass environment variables to Docker ([b3c8b8b](https://github.com/Perun-Engineering/ebpf-oom-watcher/commit/b3c8b8bb27010316cd32cd1b3d150d5496cd38ba))
* add semantic-release configuration for Rust project ([dc8516e](https://github.com/Perun-Engineering/ebpf-oom-watcher/commit/dc8516e10179d90ed1d21698bb068ead0793a586))
* add String import and clippy to pre-commit ([7d5fced](https://github.com/Perun-Engineering/ebpf-oom-watcher/commit/7d5fced4785e729458630a41a39461298d94f7db))
* correct Rust toolchain action usage for specific nightly version ([04d265b](https://github.com/Perun-Engineering/ebpf-oom-watcher/commit/04d265b2dddbe7d796d33abf937f59987c4b05c3))
* pin Rust nightly to 2024-12-01 to resolve bpfel-unknown-none target issues ([fc69a6e](https://github.com/Perun-Engineering/ebpf-oom-watcher/commit/fc69a6e75e8cc4e441f2a643da11f2349ea8a19f))
* remove clippy from pre-commit hooks ([83353df](https://github.com/Perun-Engineering/ebpf-oom-watcher/commit/83353df74b04d4fbd6f91d3489cec2e16aafb538))
* remove dry-run from semantic-release to enable actual releases ([82615f6](https://github.com/Perun-Engineering/ebpf-oom-watcher/commit/82615f6d28cc8c71c5e76c19caee7bd58b771ef1))
* remove eBPF target from pre-installation, let build handle it ([54e1754](https://github.com/Perun-Engineering/ebpf-oom-watcher/commit/54e1754027d6af122f2ef3340eb4d954d598289b))
* resolve compilation errors and add feature-based eBPF compilation ([1f82eaf](https://github.com/Perun-Engineering/ebpf-oom-watcher/commit/1f82eaf1a48e4bcadb6b23613fc24308e99aeab4))
* simplify CI toolchain setup to use nightly consistently ([fe59331](https://github.com/Perun-Engineering/ebpf-oom-watcher/commit/fe59331ee96beaa11df4add0d8f29eafc0727d65))
* skip eBPF build during cross compilation ([e757cb3](https://github.com/Perun-Engineering/ebpf-oom-watcher/commit/e757cb33ae3bd62286ffbc03b609dc5133f375c2))
* update axum API to use serve() instead of Server ([edc355a](https://github.com/Perun-Engineering/ebpf-oom-watcher/commit/edc355ab943aa353a7ad785bc4e04fb572b5c075))
* update kube-client API usage to use Config::incluster() ([6dc5345](https://github.com/Perun-Engineering/ebpf-oom-watcher/commit/6dc53453af5663a29a9b38edb60537960a2bc4d4))
* update pre-commit to use nightly fmt to match CI ([53f1906](https://github.com/Perun-Engineering/ebpf-oom-watcher/commit/53f1906aa4ce937842bad416f16b0c01b482a8b1))
* use Docker for nightly rustfmt in pre-commit hook ([ce090ce](https://github.com/Perun-Engineering/ebpf-oom-watcher/commit/ce090ce4147993b128f2fc3ed11746b3d35a1490))


### Features

* add comprehensive CI/CD workflows and development tooling ([6b44e67](https://github.com/Perun-Engineering/ebpf-oom-watcher/commit/6b44e672eb08195a26dbd56541708b360527f50c))
* add Kubernetes DaemonSet and Prometheus integration ([b8346d0](https://github.com/Perun-Engineering/ebpf-oom-watcher/commit/b8346d0cd5d7786ef5e09644689418e540784d93))
* initial commit ([a96318c](https://github.com/Perun-Engineering/ebpf-oom-watcher/commit/a96318c8482771141919645184db6f5261ae3ce4))
