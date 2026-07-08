## [0.1.10](https://github.com/Perun-Engineering/ebpf-oom-watcher/compare/v0.1.9...v0.1.10) (2026-07-08)


### Bug Fixes

* **core:** give nested eBPF build.rs cargo invocation its own target dir ([7edde0b](https://github.com/Perun-Engineering/ebpf-oom-watcher/commit/7edde0b488c9de955ceb64fd395dbd883bdef874))
* **core:** update hardcoded debug-profile eBPF object path ([07f050e](https://github.com/Perun-Engineering/ebpf-oom-watcher/commit/07f050e7953d582907dcb62f64db99700a093512))

## [0.1.9](https://github.com/Perun-Engineering/ebpf-oom-watcher/compare/v0.1.8...v0.1.9) (2026-07-08)


### Bug Fixes

* trigger release to verify native-arch parallel build pipeline ([e8dba0b](https://github.com/Perun-Engineering/ebpf-oom-watcher/commit/e8dba0b9f6a74879fc3184fef5dca9e05a2c77ed))

## [0.1.8](https://github.com/Perun-Engineering/ebpf-oom-watcher/compare/v0.1.7...v0.1.8) (2026-07-08)


### Bug Fixes

* **deps:** repair inconsistent Cargo.lock dependency tree ([258a6b6](https://github.com/Perun-Engineering/ebpf-oom-watcher/commit/258a6b6bb3cc50ec5b870f64fe803f7a3d5f346a))
* **docker:** update nightly before adding rust-src in builder stage ([a5fd9b4](https://github.com/Perun-Engineering/ebpf-oom-watcher/commit/a5fd9b4bfddf8416a1051b196758a11fe4133f3d))

## [0.1.7](https://github.com/Perun-Engineering/ebpf-oom-watcher/compare/v0.1.6...v0.1.7) (2026-07-08)


### Bug Fixes

* **docker:** re-add rust-src in builder stage to survive GHA nightly cache skew ([a292a24](https://github.com/Perun-Engineering/ebpf-oom-watcher/commit/a292a24f11bbcab1de081f5b3e61bad589333f31))

## [0.1.6](https://github.com/Perun-Engineering/ebpf-oom-watcher/compare/v0.1.5...v0.1.6) (2026-07-06)


### Bug Fixes

* bump kube from 3.1.0 to 4.0.0 ([f7cea32](https://github.com/Perun-Engineering/ebpf-oom-watcher/commit/f7cea320018762657aaf0b37dec9e0fbe3e65426))
* bump the aya group with 3 updates ([b59fd94](https://github.com/Perun-Engineering/ebpf-oom-watcher/commit/b59fd94f17bb426fbf607abbd078d15e9d3dfc78))
* **deps:** co-bump aya workspace to 0.14.0 to match aya-log 0.3.0 ([ccbacd1](https://github.com/Perun-Engineering/ebpf-oom-watcher/commit/ccbacd14b16df5c8f50077f143dc3d8154e6d74c)), closes [#42](https://github.com/Perun-Engineering/ebpf-oom-watcher/issues/42)
* **deps:** co-bump k8s-openapi to 0.28 for kube 4.0 compatibility ([4dc8117](https://github.com/Perun-Engineering/ebpf-oom-watcher/commit/4dc8117d022a03942d000513549d58c1d496bd2f))
* **deps:** remove aya async_tokio feature — removed in aya 0.14.0 ([d35db89](https://github.com/Perun-Engineering/ebpf-oom-watcher/commit/d35db89b790455a5a5512920bc9f3da99225839f))
* **ebpf:** add turbofish annotation for RingBuf::output with aya 0.2.1 ([be46be3](https://github.com/Perun-Engineering/ebpf-oom-watcher/commit/be46be3919a31f222e9f4cc27b0ab1a8393fce4e))

## [0.1.5](https://github.com/Perun-Engineering/ebpf-oom-watcher/compare/v0.1.4...v0.1.5) (2026-07-06)


### Bug Fixes

* bump anyhow from 1.0.102 to 1.0.103 ([fbaa0ba](https://github.com/Perun-Engineering/ebpf-oom-watcher/commit/fbaa0badab545b1a2c931213f47636a59feeeec2))
* bump bytes from 1.11.1 to 1.12.0 ([64df8bf](https://github.com/Perun-Engineering/ebpf-oom-watcher/commit/64df8bf3d263aead2a4fb8e1bab2fd2fc244099c))
* bump env_logger from 0.11.10 to 0.11.11 ([e8ec95f](https://github.com/Perun-Engineering/ebpf-oom-watcher/commit/e8ec95fa9b5f8979efdb18dbe96fbe2113250a72))
* bump log from 0.4.32 to 0.4.33 ([dc0bb4a](https://github.com/Perun-Engineering/ebpf-oom-watcher/commit/dc0bb4acd211305c98705abf59764ecd6bf8c87d))

## [0.1.4](https://github.com/Perun-Engineering/ebpf-oom-watcher/compare/v0.1.3...v0.1.4) (2026-07-06)


### Bug Fixes

* bump which from 8.0.3 to 8.0.4 ([0a8f169](https://github.com/Perun-Engineering/ebpf-oom-watcher/commit/0a8f1690d7e00749c4ef833d3a6091336f710f13))

## [0.1.3](https://github.com/Perun-Engineering/ebpf-oom-watcher/compare/v0.1.2...v0.1.3) (2026-06-14)


### Bug Fixes

* bump which from 6.0.3 to 8.0.3 ([#33](https://github.com/Perun-Engineering/ebpf-oom-watcher/issues/33)) ([9b2d623](https://github.com/Perun-Engineering/ebpf-oom-watcher/commit/9b2d623d3062fc91dc3da40861ec00ff295fd939))

## [0.1.2](https://github.com/Perun-Engineering/ebpf-oom-watcher/compare/v0.1.1...v0.1.2) (2026-06-14)


### Bug Fixes

* bump log from 0.4.30 to 0.4.32 ([#32](https://github.com/Perun-Engineering/ebpf-oom-watcher/issues/32)) ([a661db1](https://github.com/Perun-Engineering/ebpf-oom-watcher/commit/a661db1aed81187229754e8521528f9f2b6d1364))
* bump prometheus from 0.13.4 to 0.14.0 ([#26](https://github.com/Perun-Engineering/ebpf-oom-watcher/issues/26)) ([cf22132](https://github.com/Perun-Engineering/ebpf-oom-watcher/commit/cf221328a0f4a94412f40b378808a5c988c5bad5))
* bump regex from 1.12.3 to 1.12.4 ([#31](https://github.com/Perun-Engineering/ebpf-oom-watcher/issues/31)) ([10ddc04](https://github.com/Perun-Engineering/ebpf-oom-watcher/commit/10ddc045380cb52e38d6be90f8f14720b9dd6663))

## [0.1.1](https://github.com/Perun-Engineering/ebpf-oom-watcher/compare/v0.1.0...v0.1.1) (2026-06-01)

# [0.1.0](https://github.com/Perun-Engineering/ebpf-oom-watcher/compare/v0.0.6...v0.1.0) (2026-06-01)


### Features

* **resolve:** extract Resolution behind a ContainerResolver seam ([#23](https://github.com/Perun-Engineering/ebpf-oom-watcher/issues/23)) ([0de1b1c](https://github.com/Perun-Engineering/ebpf-oom-watcher/commit/0de1b1cafbec95dd9caac53dbd747823f5125db6))

## [0.0.6](https://github.com/Perun-Engineering/ebpf-oom-watcher/compare/v0.0.5...v0.0.6) (2026-06-01)

## [0.0.5](https://github.com/Perun-Engineering/ebpf-oom-watcher/compare/v0.0.4...v0.0.5) (2026-06-01)


### Bug Fixes

* **deps:** resolve rustls-webpki advisories and apply dependabot updates ([#21](https://github.com/Perun-Engineering/ebpf-oom-watcher/issues/21)) ([b7edc26](https://github.com/Perun-Engineering/ebpf-oom-watcher/commit/b7edc267efd26af91c42bb0352fe7d0d72a479bd))

## [0.0.4](https://github.com/Perun-Engineering/ebpf-oom-watcher/compare/v0.0.3...v0.0.4) (2026-06-01)


### Bug Fixes

* patch dependency advisories and add supply-chain CI gates ([#4](https://github.com/Perun-Engineering/ebpf-oom-watcher/issues/4)) ([323fd9d](https://github.com/Perun-Engineering/ebpf-oom-watcher/commit/323fd9dd9e5d893477e48fcc3a980724f7982c31))

## [0.0.3](https://github.com/Perun-Engineering/ebpf-oom-watcher/compare/v0.0.2...v0.0.3) (2026-06-01)


### Bug Fixes

* supervise worker tasks, scope pod query to node, repair healthcheck ([#3](https://github.com/Perun-Engineering/ebpf-oom-watcher/issues/3)) ([92fdd29](https://github.com/Perun-Engineering/ebpf-oom-watcher/commit/92fdd29dc6336597dcc011a403f8237735d1ff24))

## [0.0.2](https://github.com/Perun-Engineering/ebpf-oom-watcher/compare/v0.0.1...v0.0.2) (2026-06-01)


### Bug Fixes

* ship release binary, widen CI coverage, drop leaked transcript ([#2](https://github.com/Perun-Engineering/ebpf-oom-watcher/issues/2)) ([2f4365f](https://github.com/Perun-Engineering/ebpf-oom-watcher/commit/2f4365f9a4340662c33d11e5c5cb57db9dfffa1a))

# [1.2.0](https://github.com/Perun-Engineering/ebpf-oom-watcher/compare/v1.1.0...v1.2.0) (2025-08-14)


### Features

* initialize project with v0.0.1 baseline ([d7f05ad](https://github.com/Perun-Engineering/ebpf-oom-watcher/commit/d7f05addde5dbd7900c86764d533b3853e5fd10b))

# [1.1.0](https://github.com/Perun-Engineering/ebpf-oom-watcher/compare/v1.0.7...v1.1.0) (2025-08-14)


### Features

* enable ARM64 builds alongside AMD64 for multi-architecture support ([e329d33](https://github.com/Perun-Engineering/ebpf-oom-watcher/commit/e329d332acfd9f62fc65e4ecf6e2fb937b34f57c))

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
