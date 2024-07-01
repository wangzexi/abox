import fs from "node:fs/promises";

console.log("服务器已启动在端口 3000");

const contentTypeExtMap = new Map<string, string>([
  ["text/plain", ".txt"],
  ["text/html", ".html"],
  ["application/json", ".json"],
]);

Bun.serve({
  port: 3000,
  async fetch(req) {
    const url = new URL(req.url);

    // 首页
    if (url.pathname === "/") {
      return new Response(Bun.file("./static/index.html"));
    }

    // 子页
    const id = /^\/([\w\d]+)$/.exec(url.pathname)?.[1];
    if (id) {
      if (req.method === "GET") {
        for (const [contentType, ext] of contentTypeExtMap) {
          const file = Bun.file(`./static/pages/${id}${ext}`);
          if (!(await file.exists())) continue;

          return new Response(await file.arrayBuffer(), {
            headers: { "Content-Type": `${contentType}; charset=utf-8` },
          });
        }
      } else if (req.method === "POST") {
        // 删除可能存在的旧文件
        for (const [, ext] of contentTypeExtMap) {
          try {
            await fs.unlink(`./static/pages/${id}${ext}`);
          } catch(err) {}
        }

        // 写入新文件
        const contentType = req.headers.get("content-type") ?? "text/plain";
        const ext = contentTypeExtMap.get(contentType) ?? "";
        const path = `./static/pages/${id}${ext}`;

        await Bun.write(path, await req.arrayBuffer());
        return new Response("", {
          status: 303,
          headers: {
            Location: `/${id}`,
          },
        });
      }
    }

    return new Response("404 Not Found", { status: 404 });
  },
});
