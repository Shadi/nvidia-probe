FROM rust:1-slim AS builder
WORKDIR /build
COPY Cargo.toml Cargo.lock ./
COPY src ./src
RUN cargo build --release

FROM debian:bookworm-slim
COPY --from=builder /build/target/release/nvidia-probe /usr/local/bin/nvidia-probe
ENTRYPOINT ["nvidia-probe"]
