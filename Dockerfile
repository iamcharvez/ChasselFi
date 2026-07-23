FROM rust:1.88-bookworm AS builder
WORKDIR /build
COPY Cargo.toml ./
COPY src ./src
RUN cargo build --release

FROM debian:bookworm-slim
RUN apt-get update \
    && apt-get install -y --no-install-recommends iproute2 nftables \
    && rm -rf /var/lib/apt/lists/* \
    && useradd --system --uid 10001 --create-home chasselfi
WORKDIR /app
COPY --from=builder /build/target/release/chasselfi /usr/local/bin/chasselfi
COPY web ./web
RUN mkdir /app/data && chown -R chasselfi:chasselfi /app
USER chasselfi
EXPOSE 8080
VOLUME ["/app/data"]
ENV RUST_LOG=info
CMD ["chasselfi"]
