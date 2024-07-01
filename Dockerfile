FROM hub.zexi.me/oven/bun:latest

WORKDIR /app

COPY package.json ./
COPY bun.lockb ./
RUN bun install

COPY src ./

CMD ["bun", "start"]
