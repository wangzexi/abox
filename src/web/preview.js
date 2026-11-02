// The overlay exists only in this browser session. The short URL always serves raw bytes.
export function showPreview(data) {
  data.id = data.id || new URL(data.url).pathname.slice(1);
  const raw = `/${encodeURIComponent(data.id)}`;
  document.title = "abox";
  document.body.className = "preview-page";
  document.body.innerHTML = `
    <aside class="capsule" aria-label="分享与管理">
      <button id="copy" type="button">复制链接</button>
      ${data.key ? '<span class="capsule-divider" aria-hidden="true"></span><button id="delete" class="delete-button" type="button">删除</button>' : ""}
      <input id="share-link" readonly aria-label="分享链接" class="share-link">
      <span id="copy-status" role="status" aria-live="polite"></span>
    </aside>
    <main id="stage" class="preview-stage" aria-label="文件预览"></main>`;
  const stage = document.querySelector("#stage");
  const share = document.querySelector("#share-link");
  share.value = data.url;
  const type = data.content_type;
  if (type.startsWith("image/") && !type.startsWith("image/svg")) {
    const img = document.createElement("img");
    img.src = raw;
    img.alt = "上传图片预览";
    stage.append(img);
  } else if (type.startsWith("audio/") || type.startsWith("video/")) {
    const media = document.createElement(
      type.startsWith("audio/") ? "audio" : "video",
    );
    media.controls = true;
    media.src = raw;
    stage.append(media);
  } else if (
    type.startsWith("text/") ||
    type.startsWith("image/svg") ||
    type.startsWith("application/json") ||
    type.startsWith("application/pdf")
  ) {
    const frame = document.createElement("iframe");
    frame.title = "上传内容预览";
    frame.setAttribute(
      "sandbox",
      "allow-scripts allow-forms allow-popups allow-downloads",
    );
    frame.referrerPolicy = "no-referrer";
    frame.src = raw;
    stage.append(frame);
  } else {
    const box = document.createElement("div");
    box.className = "download-placeholder";
    box.innerHTML =
      "<strong>文件已准备好</strong><p>此类型暂不支持在线预览。<br>复制链接即可打开或分享文件。</p>";
    stage.append(box);
  }
  document.querySelector("#copy").onclick = async () => {
    const status = document.querySelector("#copy-status");
    try {
      await navigator.clipboard.writeText(share.value);
      status.textContent = "链接已复制";
    } catch {
      share.focus();
      share.select();
      status.textContent = "请按 Ctrl/Cmd + C 复制";
    }
  };
  const deleteButton = document.querySelector("#delete");
  if (deleteButton)
    deleteButton.onclick = async () => {
      if (!confirm("删除这个文件？")) return;
      deleteButton.disabled = true;
      const status = document.querySelector("#copy-status");
      try {
        const response = await fetch(raw, {
          method: "DELETE",
          headers: { Authorization: `Bearer ${data.key}` },
        });
        if (response.status === 204 || response.status === 404) {
          location.replace("/");
          return;
        }
        status.textContent = "删除失败，请重试";
      } catch {
        status.textContent = "连接中断，请重试";
      }
      deleteButton.disabled = false;
    };
  // No server route change: refreshing or sharing this address loads the actual file.
  history.pushState(null, "", raw);
  window.addEventListener("popstate", () => location.reload(), { once: true });
}
