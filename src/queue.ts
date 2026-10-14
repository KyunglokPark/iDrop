// 전송 큐 — 백엔드 이벤트를 미러링해 하단 테이블에 그린다.
// 항목이 많아도(수천) 가볍게: 전체를 Map에 들고, 변경 시 해당 행만 갱신한다.
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { t } from "./i18n";

type Dir = "upload" | "download";
type State = "queued" | "active" | "done" | "failed" | "cancelled";

interface QItem {
  id: number;
  dir: Dir;
  name: string;
  size: number;
  state: State;
  bytes: number;
  error?: string;
  row: HTMLElement;
}

const items = new Map<number, QItem>();
let body: HTMLElement;
let summary: HTMLElement;

function fmtSize(n: number): string {
  if (n < 1024) return n + " B";
  if (n < 1048576) return (n / 1024).toFixed(1) + " KB";
  if (n < 1073741824) return (n / 1048576).toFixed(1) + " MB";
  return (n / 1073741824).toFixed(2) + " GB";
}

function stateLabel(s: State): string {
  return t(`queue.state.${s}`);
}

function makeRow(it: QItem): HTMLElement {
  const row = document.createElement("div");
  row.className = "qrow";
  row.dataset.id = String(it.id);
  row.innerHTML =
    `<div class="q-dir">${it.dir === "upload" ? "⬆" : "⬇"}</div>` +
    `<div class="q-name"></div>` +
    `<div class="q-size">${fmtSize(it.size)}</div>` +
    `<div class="q-prog"><div class="q-bar"></div><span class="q-pct"></span></div>` +
    `<div class="q-state"></div>`;
  (row.querySelector(".q-name") as HTMLElement).textContent = it.name;
  row.addEventListener("click", (e) => {
    if (e.ctrlKey) row.classList.toggle("sel");
    else {
      body.querySelectorAll(".qrow.sel").forEach((r) => r.classList.remove("sel"));
      row.classList.add("sel");
    }
  });
  return row;
}

function paintRow(it: QItem) {
  const pct = it.size > 0 ? Math.min(100, Math.floor((it.bytes / it.size) * 100)) : it.state === "done" ? 100 : 0;
  (it.row.querySelector(".q-bar") as HTMLElement).style.width = pct + "%";
  (it.row.querySelector(".q-pct") as HTMLElement).textContent =
    it.state === "active" ? pct + "%" : "";
  const st = it.row.querySelector(".q-state") as HTMLElement;
  st.textContent = it.state === "failed" && it.error ? `${stateLabel("failed")}: ${it.error}` : stateLabel(it.state);
  it.row.className = "qrow state-" + it.state + (it.row.classList.contains("sel") ? " sel" : "");
}

function updateSummary() {
  let active = 0,
    queued = 0,
    done = 0,
    failed = 0;
  for (const it of items.values()) {
    if (it.state === "active") active++;
    else if (it.state === "queued") queued++;
    else if (it.state === "done") done++;
    else if (it.state === "failed") failed++;
  }
  const parts: string[] = [];
  if (active) parts.push(`${stateLabel("active")} ${active}`);
  if (queued) parts.push(`${stateLabel("queued")} ${queued}`);
  if (done) parts.push(`${stateLabel("done")} ${done}`);
  if (failed) parts.push(`${stateLabel("failed")} ${failed}`);
  summary.textContent = parts.join(" · ");
}

function selectedIds(): number[] {
  return [...body.querySelectorAll(".qrow.sel")].map((r) => Number((r as HTMLElement).dataset.id));
}

let refreshTimer: number | undefined;
function scheduleRefresh(cb: () => void) {
  clearTimeout(refreshTimer);
  refreshTimer = window.setTimeout(cb, 400);
}

export async function initQueue(onItemDone: () => void = () => {}) {
  body = document.getElementById("queue-body") as HTMLElement;
  summary = document.getElementById("queue-summary") as HTMLElement;

  document.getElementById("q-cancel")!.addEventListener("click", () => {
    const ids = selectedIds();
    if (ids.length) void invoke("queue_cancel", { ids });
  });
  document.getElementById("q-retry")!.addEventListener("click", () => {
    const ids = selectedIds();
    if (ids.length) void invoke("queue_retry", { ids });
  });
  document.getElementById("q-clear")!.addEventListener("click", () => {
    void invoke("queue_clear_done");
  });

  await listen<{ id: number; dir: Dir; name: string; size: number }[]>("queue-add", (e) => {
    const frag = document.createDocumentFragment();
    for (const v of e.payload) {
      if (items.has(v.id)) continue;
      const it: QItem = { ...v, state: "queued", bytes: 0, row: null as unknown as HTMLElement };
      it.row = makeRow(it);
      items.set(v.id, it);
      paintRow(it);
      frag.appendChild(it.row);
    }
    body.appendChild(frag);
    updateSummary();
  });

  await listen<{ id: number; state: State; error?: string }>("queue-state", (e) => {
    const it = items.get(e.payload.id);
    if (!it) return;
    it.state = e.payload.state;
    it.error = e.payload.error;
    if (it.state === "done") {
      it.bytes = it.size;
      scheduleRefresh(onItemDone);
    }
    paintRow(it);
    updateSummary();
  });

  await listen<{ active: [number, number][] }>("queue-progress", (e) => {
    for (const [id, bytes] of e.payload.active) {
      const it = items.get(id);
      if (it) {
        it.bytes = bytes;
        paintRow(it);
      }
    }
  });

  await listen("queue-cleared", () => {
    for (const [id, it] of [...items]) {
      if (it.state === "done" || it.state === "cancelled") {
        it.row.remove();
        items.delete(id);
      }
    }
    updateSummary();
  });
}
