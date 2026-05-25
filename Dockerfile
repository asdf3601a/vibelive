# Stage 1: Frontend build
FROM node:22-slim AS frontend-builder

WORKDIR /app/frontend
COPY frontend/package.json frontend/package-lock.json* ./
RUN npm ci
COPY frontend/ ./
RUN npm run build

# Stage 2: Rust build
FROM rust:1.85-slim-bookworm AS backend-builder

RUN apt-get update && apt-get install -y pkg-config libssl-dev && rm -rf /var/lib/apt/lists/*

WORKDIR /app
COPY Cargo.toml Cargo.lock* ./
COPY server/ ./server/

RUN cargo build --release --bin livestream-server

# Stage 3: Runtime
FROM debian:bookworm-slim

RUN apt-get update && apt-get install -y ca-certificates ffmpeg && rm -rf /var/lib/apt/lists/*

COPY --from=backend-builder /app/target/release/livestream-server /usr/local/bin/
COPY --from=frontend-builder /app/frontend/dist /srv/frontend

VOLUME ["/data"]
EXPOSE 1935 8080

CMD ["livestream-server"]
