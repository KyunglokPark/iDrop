// 파일 패널 — 로컬/원격 공용. FsAdapter가 경로 규칙과 백엔드 호출 차이를 흡수한다.
// 키: Enter/더블클릭(폴더 진입·파일 활성), Backspace(상위), F2(이름변경),
//     Delete(삭제), F5(새로고침), Ctrl/Shift 클릭(다중 선택)

import { t } from "./i18n";

export interface PanelEntry {
  name: string;
  isDir: boolean;
  isLink?: boolean;
  size: number;
  mtime: number; // epoch 초
  perm?: number;
  hidden?: boolean;
}

export interface FsAdapter {
  kind: "local" | "remote";
  list(path: string): Promise<[string, PanelEntry[]]>;
  mkdir(path: string): Promise<void>;
  rename(from: string, to: string): Promise<void>;
  remove(paths: string[]): Promise<void>;
  join(dir: string, name: string): string;
  parent(path: string): string;
}

export const localAdapterPaths = {
  join(dir: string, name: string): string {
    return dir.endsWith("\\") || dir.endsWith("/") ? dir + name : dir + "\\" + name;
  },
  parent(path: string): string {
    const p = path.replace(/[\\/]+$/, "");
    const i = Math.max(p.lastIndexOf("\\"), p.lastIndexOf("/"));
    if (i <= 2) return p.slice(0, 3); // "C:\" 루트
    return p.slice(0, i);
  },
};

export const remoteAdapterPaths = {
  join(dir: string, name: string): string {
    return dir.endsWith("/") ? dir + name : dir + "/" + name;
  },
  parent(path: string): string {
    const p = path.replace(/\/+$/, "");
    const i = p.lastIndexOf("/");
    return i <= 0 ? "/" : p.slice(0, i);
  },
};

type SortKey = "name" | "size" | "mtime";

function fmtSize(n: number, isDir: boolean): string {
  if (isDir) return "";
  if (n < 1024) return String(n);
  if (n < 1048576) return (n / 1024).toFixed(1) + " KB";
  if (n < 1073741824) return (n / 1048576).toFixed(1) + " MB";
  return (n / 1073741824).toFixed(2) + " GB";
}

function fmtTime(sec: number): string {
  if (!sec) return "";
  const d = new Date(sec * 1000);
  const p = (x: number) => String(x).padStart(2, "0");
  return `${d.getFullYear()}-${p(d.getMonth() + 1)}-${p(d.getDate())} ${p(d.getHours())}:${p(d.getMinutes())}`;
}

function fmtPerm(perm: number | undefined): string {
  if (perm === undefined) return "";
  const bits = "rwxrwxrwx";
  let s = "";
  for (let i = 0; i < 9; i++) s += perm & (1 << (8 - i)) ? bits[i] : "-";
  return s;
}

export class Panel {
  path = "";
  entries: PanelEntry[] = [];
  shown: PanelEntry[] = [];
  selected = new Set<string>();
  anchor = -1;
  cursor = -1; // 키보드 커서 (shown 기준 인덱스, -1 = 없음)
  showHidden = false;
  sortKey: SortKey = "name";
  sortAsc = true;
  private lastUpClick = 0;
  /** 파일/폴더 더블클릭/Enter — 반대편으로 전송 */
  onActivateNames: (names: string[]) => void = () => {};
  /** 다른 패널에서 이름들이 드롭됨 → 이 패널 경로로 전송 */
  onDropNames: (names: string[]) => void = () => {};
  /** 우클릭 메뉴 요청 (onRow: 항목 위에서 눌렀는지) */
  onContext: (x: number, y: number, onRow: boolean) => void = () => {};
  /** 목록 갱신 후 (상태 표시용) */
  onNavigated: (path: string) => void = () => {};
  private suppressClick = false; // 드래그 직후 발생하는 click 무시

  constructor(
    public adapter: FsAdapter,
    private pathInput: HTMLInputElement,
    private body: HTMLElement
  ) {
    body.classList.add("filepanel");
    body.innerHTML = "";
    this.header = document.createElement("div");
    this.header.className = "cols" + (adapter.kind === "remote" ? " remote" : "");
    const cols: [SortKey | null, string][] = [
      ["name", t("panel.name")],
      ["size", t("panel.size")],
      ["mtime", t("panel.mtime")],
    ];
    if (adapter.kind === "remote") cols.push([null, t("panel.perm")]);
    for (const [key, label] of cols) {
      const c = document.createElement("div");
      c.textContent = label;
      if (key) {
        c.classList.add("sortable");
        c.addEventListener("click", () => this.setSort(key));
      }
      this.header.appendChild(c);
    }
    this.rows = document.createElement("div");
    this.rows.className = "rows";
    body.append(this.header, this.rows);

    body.tabIndex = 0;
    body.addEventListener("keydown", (e) => this.onKey(e));
    pathInput.addEventListener("keydown", (e) => {
      if (e.key === "Enter") void this.navigate(pathInput.value);
    });

    // 빈 공간 우클릭 메뉴 (행 우클릭은 makeRow에서)
    this.rows.addEventListener("contextmenu", (ev) => {
      if ((ev.target as HTMLElement).closest(".frow")) return;
      ev.preventDefault();
      this.onContext(ev.clientX, ev.clientY, false);
    });

    Panel.all.push(this);
  }

  /** 모든 패널 (포인터 드래그의 드롭 대상 판정용) */
  static all: Panel[] = [];

  /** 패널 본문 요소 (드롭 판정·하이라이트용) */
  get el(): HTMLElement {
    return this.body;
  }

  private header: HTMLElement;
  private rows: HTMLElement;

  async navigate(path: string): Promise<void> {
    const [real, entries] = await this.adapter.list(path);
    this.path = real;
    this.entries = entries;
    this.selected.clear();
    this.anchor = -1;
    this.cursor = -1;
    this.pathInput.value = real;
    this.render();
    this.onNavigated(real);
  }

  async refresh(): Promise<void> {
    if (this.path) await this.navigate(this.path);
  }

  setSort(key: SortKey) {
    if (this.sortKey === key) this.sortAsc = !this.sortAsc;
    else {
      this.sortKey = key;
      this.sortAsc = true;
    }
    this.render();
  }

  setShowHidden(v: boolean) {
    this.showHidden = v;
    this.render();
  }

  selectedPaths(): string[] {
    return [...this.selected].map((n) => this.adapter.join(this.path, n));
  }

  selectedEntries(): PanelEntry[] {
    return this.shown.filter((e) => this.selected.has(e.name));
  }

  render() {
    const key = this.sortKey;
    const dirFirst = (a: PanelEntry, b: PanelEntry) => Number(b.isDir) - Number(a.isDir);
    const cmp = (a: PanelEntry, b: PanelEntry): number => {
      let r = 0;
      if (key === "name") r = a.name.localeCompare(b.name, undefined, { sensitivity: "base" });
      else if (key === "size") r = a.size - b.size;
      else r = a.mtime - b.mtime;
      return this.sortAsc ? r : -r;
    };
    this.shown = this.entries
      .filter((e) => this.showHidden || !(e.hidden || e.name.startsWith(".")))
      .sort((a, b) => dirFirst(a, b) || cmp(a, b));

    this.rows.innerHTML = "";
    // ".." 상위 폴더
    const up = this.makeRow({ name: "..", isDir: true, size: 0, mtime: 0 }, -1);
    this.rows.appendChild(up);
    this.shown.forEach((e, i) => this.rows.appendChild(this.makeRow(e, i)));
  }

  private makeRow(e: PanelEntry, idx: number): HTMLElement {
    const row = document.createElement("div");
    row.className = "frow" + (e.isDir ? " dir" : "") + (this.adapter.kind === "remote" ? " remote" : "");
    if (idx >= 0 && this.selected.has(e.name)) row.classList.add("sel");
    if (idx >= 0 && idx === this.cursor) row.classList.add("cursor");
    if (idx >= 0) {
      row.dataset.name = e.name;
      row.dataset.idx = String(idx);
      // 패널 간 드래그 — 포인터 기반 자체 구현.
      // (Tauri의 dragDropEnabled(탐색기→앱 드롭)가 WebView2에서 HTML5 DnD를
      //  가로채므로 HTML5 drag 이벤트는 쓸 수 없다 — iDOT 탭 드래그와 같은 방식)
      row.addEventListener("pointerdown", (ev) => {
        if (ev.button !== 0 || ev.ctrlKey || ev.shiftKey) return;
        const startX = ev.clientX;
        const startY = ev.clientY;
        let dragging = false;
        let ghost: HTMLElement | null = null;
        const names = () => (this.selected.has(e.name) ? [...this.selected] : [e.name]);
        const under = (mv: PointerEvent): Panel | null => {
          for (const p of Panel.all) {
            if (p === this) continue;
            const r = p.el.getBoundingClientRect();
            if (mv.clientX >= r.left && mv.clientX <= r.right && mv.clientY >= r.top && mv.clientY <= r.bottom) {
              return p;
            }
          }
          return null;
        };
        const onMove = (mv: PointerEvent) => {
          if (!dragging) {
            if (Math.abs(mv.clientX - startX) < 6 && Math.abs(mv.clientY - startY) < 6) return;
            dragging = true;
            if (!this.selected.has(e.name)) {
              this.selected.clear();
              this.selected.add(e.name);
              this.anchor = idx;
              this.paintSelection();
            }
            row.setPointerCapture(ev.pointerId);
            ghost = document.createElement("div");
            ghost.className = "drag-ghost";
            const n = names().length;
            ghost.textContent = n === 1 ? names()[0] : t("panel.dragN", { n });
            document.body.appendChild(ghost);
          }
          ghost!.style.left = `${mv.clientX + 12}px`;
          ghost!.style.top = `${mv.clientY + 14}px`;
          const target = under(mv);
          for (const p of Panel.all) {
            if (p !== this) p.el.classList.toggle("drop-target", p === target);
          }
        };
        const onUp = (mv: PointerEvent) => {
          row.removeEventListener("pointermove", onMove);
          row.removeEventListener("pointerup", onUp);
          row.removeEventListener("pointercancel", onUp);
          if (!dragging) return;
          this.suppressClick = true; // 드래그 끝의 click으로 선택이 바뀌지 않게
          ghost?.remove();
          const target = under(mv);
          for (const p of Panel.all) p.el.classList.remove("drop-target");
          if (target) target.onDropNames(names());
        };
        row.addEventListener("pointermove", onMove);
        row.addEventListener("pointerup", onUp);
        row.addEventListener("pointercancel", onUp);
      });
      // 우클릭 — 선택 보정 후 메뉴 요청
      row.addEventListener("contextmenu", (ev) => {
        ev.preventDefault();
        if (!this.selected.has(e.name)) {
          this.selected.clear();
          this.selected.add(e.name);
          this.anchor = idx;
          this.paintSelection();
        }
        this.onContext(ev.clientX, ev.clientY, true);
      });
    }
    const icon = e.isDir ? (e.isLink ? "🔗" : "📁") : "📄";
    const name = document.createElement("div");
    name.className = "c-name";
    name.textContent = `${icon} ${e.name}`;
    name.title = e.name;
    const size = document.createElement("div");
    size.textContent = fmtSize(e.size, e.isDir);
    const time = document.createElement("div");
    time.textContent = fmtTime(e.mtime);
    row.append(name, size, time);
    if (this.adapter.kind === "remote") {
      const perm = document.createElement("div");
      perm.textContent = fmtPerm(e.perm);
      row.appendChild(perm);
    }

    row.addEventListener("click", (ev) => {
      if (this.suppressClick) {
        this.suppressClick = false;
        return;
      }
      if (idx < 0) {
        // ".."은 클릭 한 번으로 상위 폴더 이동. 더블클릭하면 click이 2번 오므로
        // 짧은 간격의 재클릭은 무시한다 (2단계 점프 방지).
        const now = Date.now();
        if (now - this.lastUpClick < 400) return;
        this.lastUpClick = now;
        void this.navigate(this.adapter.parent(this.path)).catch(() => {});
        return;
      }
      this.cursor = idx;
      if (ev.shiftKey && this.anchor >= 0) {
        const [a, b] = [Math.min(this.anchor, idx), Math.max(this.anchor, idx)];
        if (!ev.ctrlKey) this.selected.clear();
        for (let i = a; i <= b; i++) this.selected.add(this.shown[i].name);
      } else if (ev.ctrlKey) {
        if (this.selected.has(e.name)) this.selected.delete(e.name);
        else this.selected.add(e.name);
        this.anchor = idx;
      } else {
        this.selected.clear();
        this.selected.add(e.name);
        this.anchor = idx;
      }
      // render()로 DOM을 갈아엎으면 브라우저가 더블클릭을 인식하지 못한다
      // (두 번째 클릭이 "새 요소"에 떨어짐) → 선택 클래스만 바꾼다.
      this.paintSelection();
      this.body.focus();
    });
    row.addEventListener("dblclick", () => {
      if (idx < 0) return; // ".."은 click 핸들러가 처리
      if (e.isDir) {
        void this.navigate(this.adapter.join(this.path, e.name)).catch(() => {});
      } else {
        this.onActivateNames([e.name]);
      }
    });
    return row;
  }

  /** 선택·커서 표시만 갱신 (행 DOM 유지 — 더블클릭·드래그가 끊기지 않게) */
  private paintSelection() {
    for (const el of Array.from(this.rows.children) as HTMLElement[]) {
      const name = el.dataset.name;
      if (name !== undefined) el.classList.toggle("sel", this.selected.has(name));
      const idx = el.dataset.idx;
      if (idx !== undefined) el.classList.toggle("cursor", Number(idx) === this.cursor);
    }
  }

  /** 커서를 idx로 옮기고 단일 선택, 보이도록 스크롤 */
  private moveCursor(idx: number, extendSel: boolean) {
    if (!this.shown.length) return;
    idx = Math.max(0, Math.min(this.shown.length - 1, idx));
    this.cursor = idx;
    const name = this.shown[idx].name;
    if (extendSel && this.anchor >= 0) {
      const [a, b] = [Math.min(this.anchor, idx), Math.max(this.anchor, idx)];
      this.selected.clear();
      for (let i = a; i <= b; i++) this.selected.add(this.shown[i].name);
    } else {
      this.selected.clear();
      this.selected.add(name);
      this.anchor = idx;
    }
    this.paintSelection();
    // 커서 행을 보이게 스크롤
    const row = this.rows.querySelector<HTMLElement>(`[data-idx="${idx}"]`);
    row?.scrollIntoView({ block: "nearest" });
  }

  private onKey(e: KeyboardEvent) {
    if (e.key === "ArrowDown") {
      e.preventDefault();
      this.moveCursor(this.cursor < 0 ? 0 : this.cursor + 1, e.shiftKey);
    } else if (e.key === "ArrowUp") {
      e.preventDefault();
      this.moveCursor(this.cursor < 0 ? 0 : this.cursor - 1, e.shiftKey);
    } else if (e.key === "Home") {
      e.preventDefault();
      this.moveCursor(0, e.shiftKey);
    } else if (e.key === "End") {
      e.preventDefault();
      this.moveCursor(this.shown.length - 1, e.shiftKey);
    } else if (e.key === "ArrowRight") {
      // 커서가 폴더면 진입
      e.preventDefault();
      const cur = this.cursor >= 0 ? this.shown[this.cursor] : undefined;
      if (cur?.isDir) void this.navigate(this.adapter.join(this.path, cur.name)).catch(() => {});
    } else if (e.key === "ArrowLeft" || e.key === "Backspace") {
      e.preventDefault();
      void this.navigate(this.adapter.parent(this.path)).catch(() => {});
    } else if (e.key === "F5") {
      e.preventDefault();
      void this.refresh();
    } else if (e.key === "Enter") {
      e.preventDefault();
      const sel = this.selectedEntries();
      if (sel.length === 1 && sel[0].isDir) {
        void this.navigate(this.adapter.join(this.path, sel[0].name));
      } else if (sel.length) {
        this.onActivateNames(sel.map((s) => s.name));
      }
    } else if ((e.key === "a" || e.key === "A") && e.ctrlKey) {
      e.preventDefault();
      this.shown.forEach((s) => this.selected.add(s.name));
      this.paintSelection();
    }
    // F2/Delete는 main.ts에서 대화상자와 함께 처리
  }
}
