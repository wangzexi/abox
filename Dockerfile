FROM dcr.zexi.me/oven/bun:alpine

WORKDIR /app

COPY package.json ./
COPY bun.lockb ./
RUN bun install

COPY . .

CMD ["bun", "start"]
