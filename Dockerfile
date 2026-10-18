FROM rust:1.98.1-slim-bookworm AS builder

ENV CARGO_BUILD_JOBS=2

WORKDIR /usr/src/app

RUN apt-get update && \
    apt-get install -y --no-install-recommends \
    build-essential \
    cmake \
    git \
    perl \
    golang \
    clang \
    libclang-dev \
    libssl-dev \
    pkg-config && \
    rm -rf /var/lib/apt/lists/*

RUN rustup component add rustfmt

ENV CFLAGS="-march=armv8.2-a+fp16"
ENV CXXFLAGS="-march=armv8.2-a+fp16"
ENV GGML_NATIVE=OFF
ENV GGML_CPU_ARM_ARCH=armv8.2-a+fp16

COPY Cargo.toml Cargo.lock ./

RUN mkdir src && \
    echo "fn main() {}" > src/main.rs && \
    cargo build --release

COPY src ./src

RUN touch src/main.rs && \
    cargo build --release

FROM debian:bookworm-slim
RUN apt-get update && \
    apt-get install -y --no-install-recommends ca-certificates && \
    rm -rf /var/lib/apt/lists/*

WORKDIR /usr/local/bin

COPY --from=builder /usr/src/app/target/release/omnicore_bot .
COPY --from=builder /usr/src/app/target/release/retention_audit /usr/src/app/target/release/retention_migrate ./

CMD ["./omnicore_bot"]
