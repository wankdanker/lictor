# Build from source:  docker build -f deploy/docker/Dockerfile -t lictor .
FROM rust:1-alpine AS build
RUN apk add --no-cache musl-dev git
WORKDIR /src
COPY . .
RUN cargo build --release --locked -p lictor && cp target/release/lictor /lictor

FROM gcr.io/distroless/static-debian12:nonroot
COPY --from=build /lictor /usr/bin/lictor
ENV LICTOR_HTTP_ADDR=0.0.0.0:7171 \
    LICTOR_DATA_DIR=/home/nonroot/.lictor
EXPOSE 7171
ENTRYPOINT ["/usr/bin/lictor"]
CMD ["agent"]
