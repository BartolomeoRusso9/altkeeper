# altkeeper: immagine per una macchina sempre accesa (interfaccia web + AltServer per AltStore).
#
# Compilazione (per un server x86-64, anche da un Mac con Apple Silicon):
#   docker build --platform linux/amd64 -t altkeeper:latest .
# Uso: vedi examples/docker-compose.yml. Serve la rete dell'host (network_mode: host) per
# trovare l'iPhone via Bonjour e per farsi trovare da AltStore.
#
# Nell'immagine NON c'è niente di segreto: pairing, account e stato stanno nel volume /data
# (e .dockerignore tiene fuori dalla compilazione i file che li contengono).

FROM rust:1-bookworm AS build
WORKDIR /src
COPY . .
RUN cargo build --release --locked

FROM debian:bookworm-slim
RUN apt-get update \
 && apt-get install -y --no-install-recommends ca-certificates tzdata curl \
 && rm -rf /var/lib/apt/lists/*
COPY --from=build /src/target/release/altkeeper /usr/local/bin/altkeeper
WORKDIR /data
VOLUME /data
ENTRYPOINT ["altkeeper"]
# Il PIN arriva da ALTKEEPER_WEB_PIN: senza, `serve` su 0.0.0.0 non parte.
CMD ["serve", "--bind", "0.0.0.0:8787", "--altserver"]
