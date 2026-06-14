const { invoke } = window.__TAURI__.core;
const { open } = window.__TAURI__.dialog;
const { getCurrentWebview } = window.__TAURI__.webview;

const dropzone = document.getElementById("dropzone");
const sharesEl = document.getElementById("shares");
const statusEl = document.getElementById("status");
const usePw = document.getElementById("use-pw");
const pwInput = document.getElementById("pw");
const pickBtn = document.getElementById("pick");

usePw.addEventListener("change", () => { pwInput.disabled = !usePw.checked; });

function setStatus(msg) { statusEl.textContent = msg; }

function escapeHtml(s) {
  return String(s).replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;").replace(/"/g, "&quot;").replace(/'/g, "&#x27;");
}

async function createShare(path) {
  setStatus("Starting tunnel…");
  const password = usePw.checked ? pwInput.value : null;
  try {
    const shares = await invoke("create_share", { path, password });
    render(shares);
    setStatus("");
  } catch (e) {
    setStatus("Error: " + e);
  }
}

async function revoke(token) {
  const shares = await invoke("revoke_share", { token });
  render(shares);
}

function formatSize(bytes) {
  const units = ["B", "KB", "MB", "GB"];
  let size = bytes, u = 0;
  while (size >= 1024 && u < units.length - 1) { size /= 1024; u++; }
  return `${size.toFixed(1)} ${units[u]}`;
}

function render(shares) {
  sharesEl.innerHTML = "";
  for (const s of shares) {
    const li = document.createElement("li");
    const link = s.link ?? null;
    const linkHtml = link
      ? `<a class="link" href="${escapeHtml(link)}" target="_blank" rel="noopener noreferrer">${escapeHtml(link)}</a>`
      : `<div class="link">(link pending…)</div>`;
    li.innerHTML = `
      <div class="name">${escapeHtml(s.name)} <small>(${formatSize(s.size)})</small></div>
      ${linkHtml}
      <div>
        <button class="copy" ${link ? "" : "disabled"}>Copy link</button>
        <button class="revoke">Revoke</button>
        <small>${s.has_password ? "🔒 " : ""}${s.download_count} downloads</small>
      </div>`;
    li.querySelector(".copy").onclick = () => { if (link) navigator.clipboard.writeText(link); };
    li.querySelector(".revoke").onclick = () => revoke(s.token);
    sharesEl.appendChild(li);
  }
}

pickBtn.addEventListener("click", async () => {
  const path = await open({ multiple: false });
  if (typeof path === "string") createShare(path);
});

// Tauri file drop events.
getCurrentWebview().onDragDropEvent((event) => {
  if (event.payload.type === "over") {
    dropzone.classList.add("over");
  } else if (event.payload.type === "drop") {
    dropzone.classList.remove("over");
    const paths = event.payload.paths;
    if (paths && paths.length > 0) createShare(paths[0]);
  } else {
    dropzone.classList.remove("over");
  }
});

// Initial render.
invoke("list_shares").then(render);
