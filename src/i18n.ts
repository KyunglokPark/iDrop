// iDrop UI 문자열 (한국어 / English) — iDOT i18n.ts와 같은 API.
//   data-i18n="key"             → textContent
//   data-i18n-title="key"       → title 속성
//   data-i18n-placeholder="key" → placeholder 속성
// 동적 문자열은 t("key", {vars})로 가져온다. 문자열은 마일스톤 진행에 따라 추가.

export type Lang = "ko" | "en";

export const LANG_KEY = "idrop.lang";

function readStored(): Lang {
  try {
    return localStorage.getItem(LANG_KEY) === "en" ? "en" : "ko";
  } catch {
    return "ko";
  }
}

let lang: Lang = readStored();

export function getLang(): Lang {
  return lang;
}

export function setLang(l: Lang) {
  lang = l;
  try {
    localStorage.setItem(LANG_KEY, l);
  } catch {
    /* ignore */
  }
}

const STRINGS: Record<string, { ko: string; en: string }> = {
  // ---- 상단바 / 퀵커넥트 ----
  "top.sites": { ko: "사이트", en: "Sites" },
  "top.host": { ko: "호스트", en: "Host" },
  "top.port": { ko: "포트", en: "Port" },
  "top.user": { ko: "사용자", en: "User" },
  "top.pass": { ko: "비밀번호", en: "Password" },
  "top.connect": { ko: "빠른 연결", en: "Quickconnect" },
  "top.disconnect": { ko: "연결 끊기", en: "Disconnect" },

  // ---- 패널 ----
  "panel.local": { ko: "로컬", en: "Local" },
  "panel.remote": { ko: "원격", en: "Remote" },
  "panel.name": { ko: "이름", en: "Name" },
  "panel.size": { ko: "크기", en: "Size" },
  "panel.mtime": { ko: "수정한 날짜", en: "Modified" },
  "panel.perm": { ko: "권한", en: "Perms" },
  "panel.refresh": { ko: "새로 고침 (F5)", en: "Refresh (F5)" },
  "panel.hidden": { ko: "숨김 파일 표시", en: "Show hidden files" },
  "panel.notConnected": { ko: "연결되어 있지 않습니다", en: "Not connected" },

  "panel.newFolder": { ko: "새 폴더", en: "New folder" },
  "panel.newFolderTitle": { ko: "새 폴더 이름", en: "New folder name" },
  "panel.renameTitle": { ko: "이름 바꾸기 (F2)", en: "Rename (F2)" },
  "panel.deleteTitle": { ko: "삭제", en: "Delete" },
  "panel.deleteMsg": {
    ko: "{n}개 항목을 영구 삭제할까요? (휴지통을 거치지 않습니다)",
    en: "Permanently delete {n} item(s)? (does not use the Recycle Bin)",
  },
  "panel.transferLater": { ko: "전송 기능은 준비 중입니다", en: "Transfer is coming soon" },
  // ---- 전송 큐 ----
  "queue.title": { ko: "전송 큐", en: "Transfer queue" },
  "queue.cancel": { ko: "선택 취소", en: "Cancel selected" },
  "queue.retry": { ko: "재시도", en: "Retry" },
  "queue.clear": { ko: "완료 항목 지우기", en: "Clear finished" },
  "queue.state.queued": { ko: "대기", en: "Queued" },
  "queue.state.active": { ko: "전송 중", en: "Active" },
  "queue.state.done": { ko: "완료", en: "Done" },
  "queue.state.failed": { ko: "실패", en: "Failed" },
  "queue.state.cancelled": { ko: "취소됨", en: "Cancelled" },
  "queue.state.skipped": { ko: "건너뜀", en: "Skipped" },


  // ---- 대화상자 ----
  "dlg.ok": { ko: "확인", en: "OK" },
  "dlg.cancel": { ko: "취소", en: "Cancel" },
  "hk.title": { ko: "호스트 키 확인", en: "Verify host key" },
  "hk.changedTitle": { ko: "호스트 키가 바뀌었습니다!", en: "Host key changed!" },
  "hk.changedWarn": {
    ko: "저장된 키와 다릅니다. 중간자 공격이거나 서버가 재설치된 것일 수 있습니다.",
    en: "Differs from the saved key. This could be a MITM attack or a reinstalled server.",
  },
  "hk.save": { ko: "저장하고 접속", en: "Save & connect" },
  "hk.once": { ko: "이번만 접속", en: "Connect once" },
  "auth.user": { ko: "사용자명 입력", en: "Enter user name" },
  "auth.password": { ko: "비밀번호 입력", en: "Enter password" },
  "auth.passphrase": { ko: "키 암호문구 입력", en: "Enter key passphrase" },
  // ---- 상태 ----
  "status.connecting": { ko: "접속 중: {host}", en: "Connecting: {host}" },
  "status.connected": { ko: "접속됨: {host}", en: "Connected: {host}" },
  "status.disconnected": { ko: "연결 끊김", en: "Disconnected" },
};

export function t(key: string, vars?: Record<string, string | number>): string {
  const s = STRINGS[key];
  let out = s ? s[lang] : key;
  if (vars) {
    for (const [k, v] of Object.entries(vars)) out = out.replaceAll(`{${k}}`, String(v));
  }
  return out;
}

export function applyI18n(root: ParentNode = document) {
  root.querySelectorAll<HTMLElement>("[data-i18n]").forEach((el) => {
    el.textContent = t(el.dataset.i18n!);
  });
  root.querySelectorAll<HTMLElement>("[data-i18n-title]").forEach((el) => {
    el.title = t(el.dataset.i18nTitle!);
  });
  root.querySelectorAll<HTMLInputElement>("[data-i18n-placeholder]").forEach((el) => {
    el.placeholder = t(el.dataset.i18nPlaceholder!);
  });
}
