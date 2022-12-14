FROM rust:1.65-buster as builder
WORKDIR /usr/src/myapp
RUN apt update
RUN apt install -y mosquitto-clients cmake
COPY src src
COPY Cargo.toml .
#RUN cargo build . --release
RUN cargo install --path .

FROM debian:buster-slim
RUN apt update
RUN apt install -y mosquitto-clients cmake
COPY --from=builder /usr/local/cargo/bin/sitewhere-controller /usr/local/bin/sitewhere-controller

CMD ["sitewhere-controller"]