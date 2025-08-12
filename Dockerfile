FROM rust:latest

RUN apt-get update &&     apt-get install -y clang llvm libclang-dev build-essential pkg-config musl-tools git libelf-dev linux-image-generic bpftool

RUN bpftool btf dump file /sys/kernel/btf/vmlinux format c > /usr/include/vmlinux.h

RUN cargo install bpf-linker

WORKDIR /workspace
COPY . .

# Only set the linker for the eBPF target
ENV CARGO_TARGET_BPFEL_UNKNOWN_NONE_LINKER=bpf-linker

RUN rustup component add rust-src --toolchain nightly

CMD ["bash"]
