# 构建阶段 (alpine 默认使用 musl, 产出完全静态的二进制)
FROM rust:1-alpine AS build
RUN apk add --no-cache musl-dev
WORKDIR /app
COPY Cargo.toml Cargo.lock ./
COPY src ./src
RUN cargo build --release

# 运行阶段: scratch, 只包含静态二进制
FROM scratch
COPY --from=build /app/target/release/abox /abox
EXPOSE 3000
CMD ["/abox"]
