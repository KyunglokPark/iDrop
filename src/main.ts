// iDrop — 앱 셸. 퀵커넥트 + 접속 상태. (패널/큐는 M2·M3에서 모듈로 교체)
import "./styles.css";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { applyI18n, getLang, t } from "./i18n";
import { initDialogs } from "./dialogs";

applyI18n();
void invoke("set_lang", { lang: getLang() }).catch(() => {});
await initDialogs();

const statusEl = document.getElementById("status") as HTMLElement;
const qcHost = document.getElementById("qc-host") as HTMLInputElement;
const qcPort = document.getElementById("qc-port") as HTMLInputElement;
const qcUser = document.getElementById("qc-user") as HTMLInputElement;
const qcPass = document.getElementById("qc-pass") as HTMLInputElement;
const qcBtn = document.getElementById("qc-connect") as HTMLButtonElement;
const remotePath = document.getElementById("remote-path") as HTMLInputElement;
const remoteList = document.getElementById("remote-list") as HTMLElement;
const remoteRefresh = document.getElementById("remote-refresh") as HTMLButtonElement;

export function setStatus(msg: string) {
  statusEl.textContent = msg;
}

export interface RemoteEntry {
  name: string;
  isDir: boolean;
  isLink: boolean;
  size: number;
  mtime: number;
  perm: number;
}

let connected = false;

// ---- 임시 원격 목록 렌더링 (M2에서 panel.ts로 교체) --------------------------

function fmtSize(n: number): string {
  if (n < 1024) return String(n);
  if (n < 1024 * 1024) return (n / 1024).toFixed(1) + " KB";
  if (n < 1024 * 1024 * 1024) return (n / 1024 / 1024).toFixed(1) + " MB";
  return (n / 1024 / 1024 / 1024).toFixed(2) + " GB";
}

async function listRemote(path: string) {
  const [real, entries] = await invoke<[string, RemoteEntry[]]>("remote_list", { path });
  remotePath.value = real;
  remoteList.innerHTML = "";
  const up = document.createElement("div");
  up.className = "row dir";
  up.textContent = "📁 ..";
  up.ondblclick = () => void listRemote(real.replace(/\/[^/]+\/?$/, "") || "/");
  remoteList.appendChild(up);
  for (const en of entries) {
    const row = document.createElement("div");
    row.className = "row" + (en.isDir ? " dir" : "");
    row.textContent = `${en.isDir ? "📁" : "📄"} ${en.name}${en.isDir ? "" : "  (" + fmtSize(en.size) + ")"}`;
    if (en.isDir) {
      row.ondblclick = () => void listRemote(real.replace(/\/$/, "") + "/" + en.name);
    }
    remoteList.appendChild(row);
  }
}

// ---- 퀵커넥트 ---------------------------------------------------------------

async function quickConnect() {
  if (connected) {
    await invoke("site_disconnect").catch(() => {});
    return;
  }
  const host = qcHost.value.trim();
  if (!host) {
    qcHost.focus();
    return;
  }
  qcBtn.disabled = true;
  try {
    const home = await invoke<string>("site_connect", {
      hop: {
        host,
        port: parseInt(qcPort.value, 10) || 22,
        username: qcUser.value.trim(),
        password: qcPass.value,
        keyFile: "",
        useAgent: false,
      },
      encoding: "auto",
    });
    await listRemote(home);
  } catch (err) {
    setStatus(String(err));
  } finally {
    qcBtn.disabled = false;
  }
}

qcBtn.addEventListener("click", () => void quickConnect());
for (const el of [qcHost, qcPort, qcUser, qcPass]) {
  el.addEventListener("keydown", (e) => {
    if (e.key === "Enter") void quickConnect();
  });
}
remoteRefresh.addEventListener("click", () => {
  if (connected) void listRemote(remotePath.value);
});
remotePath.addEventListener("keydown", (e) => {
  if (e.key === "Enter" && connected) void listRemote(remotePath.value);
});

// ---- 접속 상태 ---------------------------------------------------------------

await listen<{ state: string; host: string; home?: string; error?: string }>(
  "site-state",
  (e) => {
    const p = e.payload;
    if (p.state === "connecting") {
      setStatus(t("status.connecting", { host: p.host }));
    } else if (p.state === "connected") {
      connected = true;
      qcBtn.textContent = t("top.disconnect");
      setStatus(t("status.connected", { host: p.host }));
    } else {
      connected = false;
      qcBtn.textContent = t("top.connect");
      setStatus(p.error ? p.error : t("status.disconnected"));
      remoteList.innerHTML = `<div class="placeholder">${t("panel.notConnected")}</div>`;
      remotePath.value = "";
    }
  }
);
