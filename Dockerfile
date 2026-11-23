FROM docker.io/library/rust:1 AS toolchain
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
COPY docs/nginx docs/nginx
WORKDIR /workspace/server
RUN cargo build --release -p app -p ctl \
    && mkdir /out-data

FROM cgr.dev/chainguard/glibc-dynamic:latest@sha256:2c362104c7f7adc37e6bdc87451450ff3834871a6268d930815cbe561493f947 AS runtime
COPY --from=server --chown=65532:65532 /out-data /data
COPY --from=server /workspace/server/target/release/homespace /usr/local/bin/homespace
COPY --from=server /workspace/server/target/release/homespace-ctl /usr/local/bin/homespace-ctl
COPY --from=web /workspace/clients/web/dist /srv/web
USER 65532:65532
ENV HOMESPACE_BIND=0.0.0.0:8080 \
    HOMESPACE_STORAGE=local \
    HOMESPACE_STORAGE_ROOT=/data \
    HOMESPACE_WEB_ROOT=/srv/web
EXPOSE 8080
VOLUME /data
ENTRYPOINT ["homespace"]
