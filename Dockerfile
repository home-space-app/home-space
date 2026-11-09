FROM docker.io/library/rust:latest AS toolchain
RUN rustup target add wasm32-unknown-unknown \
    && cargo install trunk --locked
WORKDIR /workspace

FROM toolchain AS web
COPY shared shared
COPY clients clients
WORKDIR /workspace/clients/web
RUN trunk build --release

FROM toolchain AS server
COPY shared shared
COPY server server
WORKDIR /workspace/server
RUN cargo build --release -p app

FROM docker.io/library/debian:stable-slim AS runtime
RUN useradd --system --create-home --uid 10001 homespace \
    && mkdir /data \
    && chown homespace /data
COPY --from=server /workspace/server/target/release/homespace /usr/local/bin/homespace
COPY --from=web /workspace/clients/web/dist /srv/web
USER homespace
ENV HOMESPACE_BIND=0.0.0.0:8080 \
    HOMESPACE_STORAGE=local \
    HOMESPACE_STORAGE_ROOT=/data \
    HOMESPACE_WEB_ROOT=/srv/web
EXPOSE 8080
VOLUME /data
ENTRYPOINT ["homespace"]
CMD ["serve"]

FROM toolchain AS test
CMD ["sh", "-c", "cd server && cargo test --workspace --all-features && cd ../clients && cargo test --workspace"]
