FROM rust:1.99-slim-bookworm AS builder

WORKDIR /app
COPY Cargo.toml Cargo.lock ./
COPY src ./src
COPY templates ./templates
COPY static ./static
RUN cargo build --release --locked

FROM debian:bookworm-slim

RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates wget \
    && rm -rf /var/lib/apt/lists/*

RUN useradd --system --uid 10001 --create-home lstock

WORKDIR /app
COPY --from=builder /app/target/release/l_stock /usr/local/bin/l_stock
COPY --from=builder /app/static /app/static
COPY --from=builder /app/templates /app/templates
RUN mkdir -p /data && chown -R lstock:lstock /data /app

USER lstock
ENV DATA_DIR=/data \
    DB_PATH=/data/l_stock.db \
    BIND_ADDR=0.0.0.0:8080
EXPOSE 8080
ENTRYPOINT ["l_stock"]
CMD ["serve"]
