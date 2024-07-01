import fs from "node:fs/promises";
import mime from "mime-types";

console.log("服务器已启动在端口 3000");

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
        const files = await fs.readdir("./static/pages");
        for (const file of files) {
          if (!file.startsWith(id)) continue;

          const contentType = mime.lookup(file) || "text/plain";
          const headers: Record<string, string> = {
            "Content-Type": `${contentType}; charset=utf-8`,
          };

          if (
            ![
              "text",
              "image",
              "audio",
              "video",
              "application/pdf",
              "application/json",
            ].some((type) => contentType.startsWith(type))
          ) {
            headers[
              "Content-Disposition"
            ] = `attachment; filename="${id}.${mime.extension(contentType)}"`;
          }

          return new Response(await fs.readFile(`./static/pages/${file}`), {
            headers,
          });
        }
      } else if (req.method === "POST") {
        // 删除旧文件
        const files = await fs.readdir("./static/pages");
        for (const file of files) {
          if (!file.startsWith(id)) continue;
          await fs.unlink(`./static/pages/${file}`);
        }

        // 写入新文件
        const contentType = req.headers.get("Content-Type") ?? "text/plain";
        const fileExt = req.headers.get("X-File-Extension") || mime.extension(contentType) || "";
        const path = `./static/pages/${id}${fileExt ? `.${fileExt}` : ""}`;
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
