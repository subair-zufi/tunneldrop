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

// Tokens (and names) of the shares currently on screen, so we can tell when one
// disappears between renders — i.e. it was revoked here, from another window, or
// its tunnel went away — and announce it instead of having it silently vanish.
let knownShares = new Map();
let revokedTimer = null;

function notifyRevoked(names) {
  if (names.length === 0) return;
  const label = names.length === 1
    ? `“${names[0]}” was revoked and is no longer available.`
    : `${names.length} shares were revoked.`;
  setStatus(`🚫 ${label}`);
  clearTimeout(revokedTimer);
  revokedTimer = setTimeout(() => {
    if (statusEl.textContent.startsWith("🚫")) setStatus("");
  }, 5000);
}

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
  const current = new Map();
  for (const s of shares) {
    current.set(s.token, s.name);
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

  // Anything we knew about but that is gone now was revoked — surface it.
  const revoked = [];
  for (const [token, name] of knownShares) {
    if (!current.has(token)) revoked.push(name);
  }
  knownShares = current;
  notifyRevoked(revoked);
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

// Auto-refresh the list so download counts (and pending links) update without
// any user action. Guarded so a slow/failed call never stacks up overlapping
// refreshes. Paused while the window is hidden to avoid needless work.
let refreshing = false;
async function refresh() {
  if (refreshing || document.hidden) return;
  refreshing = true;
  try {
    render(await invoke("list_shares"));
  } catch (_) {
    // Ignore transient errors; the next tick will retry.
  } finally {
    refreshing = false;
  }
}

// Initial render, then poll every 5 seconds.
refresh();
setInterval(refresh, 5000);
// Refresh immediately when the window regains focus.
document.addEventListener("visibilitychange", () => { if (!document.hidden) refresh(); });

// ── Self-update ──────────────────────────────────────────────────────────────
// Uses the Tauri updater plugin, which fetches a signed static manifest from
// GitHub Releases (no API/server of ours), verifies it against the public key
// baked into the app, downloads the new bundle and installs it — then we
// relaunch. All feature-detected so the page still works in a plain browser
// (e.g. the serve_local example) where the plugins aren't present.
const updateBanner = document.getElementById("update-banner");
const updateText = document.getElementById("update-text");
const updateInstall = document.getElementById("update-install");
const updateDismiss = document.getElementById("update-dismiss");
const checkUpdatesBtn = document.getElementById("check-updates");

let pendingUpdate = null;

function showUpdateBanner(update) {
  pendingUpdate = update;
  updateText.textContent = `Update available: v${update.version}`;
  updateBanner.hidden = false;
}

async function installUpdate() {
  if (!pendingUpdate) return;
  updateInstall.disabled = true;
  try {
    let total = 0, received = 0;
    await pendingUpdate.downloadAndInstall((ev) => {
      switch (ev.event) {
        case "Started":
          total = ev.data?.contentLength ?? 0;
          setStatus("Downloading update…");
          break;
        case "Progress":
          received += ev.data?.chunkLength ?? 0;
          setStatus(total
            ? `Downloading update… ${Math.round((received / total) * 100)}%`
            : "Downloading update…");
          break;
        case "Finished":
          setStatus("Installing update…");
          break;
      }
    });
    // Restart into the freshly installed version.
    await window.__TAURI__.process.relaunch();
  } catch (e) {
    setStatus("Update failed: " + e);
    updateInstall.disabled = false;
  }
}

// `manual` = triggered by the user; surfaces "you're up to date" / errors that
// we stay silent about on the automatic startup check.
async function checkForUpdates(manual = false) {
  const updater = window.__TAURI__?.updater;
  if (!updater) {
    if (manual) setStatus("Updates aren't available in this build.");
    return;
  }
  try {
    if (manual) setStatus("Checking for updates…");
    const update = await updater.check();
    if (update) {
      showUpdateBanner(update);
      if (manual) setStatus("");
    } else if (manual) {
      setStatus("You're on the latest version.");
    }
  } catch (e) {
    if (manual) setStatus("Update check failed: " + e);
  }
}

updateInstall.addEventListener("click", installUpdate);
updateDismiss.addEventListener("click", () => { updateBanner.hidden = true; });
checkUpdatesBtn.addEventListener("click", () => checkForUpdates(true));

// Silent check shortly after launch so a waiting update surfaces on its own.
setTimeout(() => checkForUpdates(false), 3000);
