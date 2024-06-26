console.log("服务器已启动在端口 3000");

Bun.serve({
  port: 3000,
  async fetch(req) {
    const url = new URL(req.url);
    if (url.pathname === "/") {
      return new Response(Bun.file("./static/index.html"));
    }

    const id = /^\/([\w\d]+)$/.exec(url.pathname)?.[1];
    if (id) {
      const path = `./static/pages/${id}`;

      if (req.method === "GET") {
        const file = Bun.file(path);
        if (await file.exists()) {
          const text = await file.text()
          
          let contentType: string = 'text/plain'
          
          if (/<(\w+)>.+<\/\1>/.test(text)) {
            contentType = 'text/html'
          }

          try {
            JSON.parse(text)
            contentType = 'application/json'
          } catch (error) {}
          
          return new Response(
            file,
            {
              headers: { "Content-Type": `${contentType}; charset=utf-8` },
            }
          );
        }
      } else if (req.method === "POST") {
        if (id) {
          await Bun.write(path, await req.text());
          return new Response("", {
            status: 303,
            headers: {
              Location: `/${id}`,
            },
          });
        }
      }
    }

    return new Response("404 Not Found", { status: 404 });
  },
});
