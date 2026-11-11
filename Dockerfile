# --- Этап 1: Сборка Vue ---
FROM node:20-alpine AS frontend-builder
WORKDIR /app/cashcode-ui
COPY cashcode-ui/package*.json ./
RUN npm ci
COPY cashcode-ui/ ./
RUN npm run build

# --- Этап 2: Сборка Rust ---
FROM rust:1.85-slim AS backend-builder
WORKDIR /app

# Устанавливаем системные зависимости, необходимые для сборки
RUN apt-get update && apt-get install -y \
    pkg-config \
    libudev-dev \
    && rm -rf /var/lib/apt/lists/*

COPY Cargo.toml Cargo.lock ./
COPY src ./src
RUN cargo build --release

# --- Этап 3: Финальный образ ---
FROM debian:bookworm-slim
WORKDIR /app
# Копируем бинарник
COPY --from=backend-builder /app/target/release/cashcode-web /usr/local/bin/
# Копируем собранный Vue (папка dist)
COPY --from=frontend-builder /app/cashcode-ui/dist ./cashcode-ui/dist
CMD ["cashcode-web"]