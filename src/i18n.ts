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
  "panel.dragN": { ko: "{n}개 항목", en: "{n} items" },
  // ---- 우클릭 메뉴 ----
  "menu.upload": { ko: "업로드", en: "Upload" },
  "menu.download": { ko: "다운로드", en: "Download" },
  "menu.edit": { ko: "편집", en: "Edit" },
  "menu.rename": { ko: "이름 바꾸기", en: "Rename" },
  "menu.delete": { ko: "삭제", en: "Delete" },
  "menu.refresh": { ko: "새로 고침", en: "Refresh" },
  // ---- 원격 편집 상태 ----
  "edit.opened": { ko: "편집 중: {name} (저장하면 자동 업로드)", en: "Editing: {name} (saves upload automatically)" },
  "edit.saved": { ko: "업로드됨: {name}", en: "Uploaded: {name}" },
  "edit.error": { ko: "편집 업로드 실패: {name} — {error}", en: "Edit upload failed: {name} — {error}" },
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
  // ---- 사이트 관리자 ----
  "sites.title": { ko: "사이트 관리자", en: "Site Manager" },
  "sites.new": { ko: "＋ 새 사이트", en: "+ New site" },
  "sites.newGroup": { ko: "＋ 새 그룹", en: "+ New group" },
  "sites.newGroupTitle": { ko: "새 그룹 이름", en: "New group name" },
  "sites.importIdot": { ko: "iDOT에서 가져오기", en: "Import from iDOT" },
  "sites.importFile": { ko: "파일 가져오기", en: "Import file" },
  "sites.copySite": { ko: "사이트 복사", en: "Copy site" },
  "sites.copyGroup": { ko: "그룹 복사", en: "Copy group" },
  "sites.renameGroup": { ko: "그룹 이름 바꾸기", en: "Rename group" },
  "sites.deleteGroup": { ko: "그룹 삭제", en: "Delete group" },
  "sites.deleteGroupConfirm": {
    ko: "\"{name}\" 그룹을 삭제할까요? 소속 사이트 {n}개는 그룹 없음으로 이동합니다.",
    en: "Delete group \"{name}\"? Its {n} site(s) move to ungrouped.",
  },
  "sites.deleteSiteConfirm": { ko: "\"{name}\" 사이트를 삭제할까요?", en: "Delete site \"{name}\"?" },
  "sites.empty": { ko: "저장된 사이트가 없습니다", en: "No saved sites" },
  "sites.import": { ko: "가져오기", en: "Import" },
  "sites.name": { ko: "이름", en: "Name" },
  "sites.group": { ko: "그룹", en: "Group" },
  "sites.protocol": { ko: "프로토콜", en: "Protocol" },
  "sites.enc": { ko: "파일명 인코딩", en: "Filename encoding" },
  "sites.save": { ko: "저장", en: "Save" },
  "sites.connect": { ko: "접속", en: "Connect" },
  "sites.delete": { ko: "삭제", en: "Delete" },
  "sites.importGroup": { ko: "가져올 그룹 이름", en: "Group name for imported sites" },
  "sites.importNone": { ko: "가져올 SSH 세션이 없습니다", en: "No SSH sessions to import" },
  "sites.importDone": { ko: "가져오기 완료", en: "Import complete" },
  "sites.importedN": { ko: "{n}개 가져옴 ({skipped}개 제외)", en: "Imported {n} ({skipped} skipped)" },
  // ---- 충돌 ----
  "conflict.title": { ko: "같은 이름이 있습니다", en: "File already exists" },
  "conflict.msg": { ko: "\"{name}\" 이(가) 이미 있습니다. 어떻게 할까요?", en: "\"{name}\" already exists. What would you like to do?" },
  "conflict.overwrite": { ko: "덮어쓰기", en: "Overwrite" },
  "conflict.rename": { ko: "이름 바꿔 저장", en: "Rename" },
  "conflict.skip": { ko: "건너뛰기", en: "Skip" },
  "conflict.applyAll": { ko: "모두 적용", en: "Apply to all" },
  // ---- 상태 ----
  "status.connecting": { ko: "접속 중: {host}", en: "Connecting: {host}" },
  "status.connected": { ko: "접속됨: {host}", en: "Connected: {host}" },
  "status.disconnected": { ko: "연결 끊김", en: "Disconnected" },
  // ---- 도움말 ----
  "top.help": { ko: "도움말", en: "Help" },
  "help.title": { ko: "아이드롭 도움말", en: "iDrop Help" },
  "close.esc": { ko: "닫기 (Esc)", en: "Close (Esc)" },
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

// ---- 도움말 본문 -------------------------------------------------------------

const BUG_MAIL = "nezzard@idoits.co.kr";

const NOTICE_KO = `
<div class="help-notice">
  <p>iDrop(아이드롭)의 모든 저작권은 <b>(주)이도아이티</b>에 있습니다. 본 소프트웨어와 그 구성 요소(프로그램, 디자인, 로고, 문서 등)는 저작권법의 보호를 받으며, (주)이도아이티의 사전 서면 동의 없이 복제·수정·배포하거나 상업적으로 이용할 수 없습니다.</p>
  <p class="copy">© 2026 (주)이도아이티. All rights reserved.</p>
  <p>버그 문의: <span class="mail">${BUG_MAIL}</span></p>
</div>
`;

const NOTICE_EN = `
<div class="help-notice">
  <p>All copyrights to iDrop belong to <b>IDOIT Co., Ltd.</b> This software and its components (program, design, logo and documentation) are protected by copyright law and may not be copied, modified, distributed or used commercially without the prior written consent of IDOIT Co., Ltd.</p>
  <p class="copy">© 2026 IDOIT Co., Ltd. All rights reserved.</p>
  <p>Bug reports: <span class="mail">${BUG_MAIL}</span></p>
</div>
`;

const HELP_KO = `
<h3>접속</h3>
<ul>
  <li><b>빠른 연결</b>: 상단바에서 프로토콜(SFTP·FTP)·호스트·포트·사용자·비밀번호를 입력하고 <b>빠른 연결</b>을 누르거나 <kbd>Enter</kbd>로 바로 접속합니다. 접속 중에는 같은 버튼이 <b>연결 끊기</b>로 바뀝니다.</li>
  <li><b>사이트 관리자</b>: <b>사이트</b> 버튼으로 엽니다. 서버를 저장하고 <b>그룹</b>으로 묶어 관리하며, 목록에서 <b>더블클릭</b>하거나 <b>접속</b> 버튼으로 연결합니다. 끌어서 순서를 바꾸거나 그룹에 넣고, 빈 곳에 놓으면 그룹에서 빠집니다.</li>
  <li>항목에 마우스를 올리면 나오는 버튼으로 <b>이름 변경(✎)·복사(⧉)·삭제(✕)</b>를 합니다. 그룹을 복사하면 소속 사이트도 함께 복사됩니다.</li>
  <li><b>iDOT에서 가져오기</b>: 같은 PC의 iDOT 접속 목록을 그대로(그룹·비밀번호 포함) 가져옵니다. <b>파일 가져오기</b>로 iDrop·iDOT CSV, Xshell 세션(<code>.xsh</code>·<code>.xts</code>)도 불러옵니다. 이미 있는 사이트는 건너뜁니다.</li>
  <li><b>파일명 인코딩</b>: EUC-KR 서버의 한글 파일 이름을 위해 <b>자동</b>·UTF-8·EUC-KR을 고릅니다. 자동이 대부분 맞습니다.</li>
</ul>

<h3>파일 탐색</h3>
<ul>
  <li>왼쪽은 <b>로컬</b>(내 PC), 오른쪽은 <b>원격</b>(서버)입니다. <b>더블클릭</b>으로 폴더에 들어가고, 경로 칸에 직접 입력해 이동하며, <kbd>Backspace</kbd>로 상위 폴더로 올라갑니다.</li>
  <li>로컬 패널의 <b>드라이브</b> 선택으로 디스크를 바꿉니다. 각 패널의 <b>📁+</b> 새 폴더, <b>👁</b> 숨김 파일 표시, <b>⟳</b>(또는 <kbd>F5</kbd>) 새로고침입니다.</li>
  <li>열 제목(이름·크기·수정일)을 눌러 <b>정렬</b>하고, 다시 누르면 오름/내림차순이 바뀝니다.</li>
  <li><kbd>Ctrl</kbd>·<kbd>Shift</kbd>+클릭으로 <b>여러 개</b>를 선택합니다.</li>
</ul>

<h3>전송</h3>
<ul>
  <li>파일·폴더를 <b>한쪽 패널에서 다른 쪽으로 끌어다 놓으면</b> 전송됩니다. 원격→로컬은 <b>다운로드</b>, 로컬→원격은 <b>업로드</b>입니다. 원격 파일을 <b>더블클릭</b>해도 받을 수 있습니다.</li>
  <li>전송은 아래 <b>전송 큐</b>에 쌓여 순서대로 처리되고 진행률이 표시됩니다. 큐 머리의 버튼으로 <b>취소(✕)·재시도(↻)·완료 항목 비우기(🧹)</b>를 합니다.</li>
  <li>같은 이름의 파일이 있으면 <b>덮어쓰기 / 이름 바꿔 저장 / 건너뛰기</b>를 묻고, <b>모두 적용</b>으로 나머지에도 같은 선택을 적용합니다.</li>
  <li>폴더를 통째로 올리거나 받으면 하위 폴더·파일까지 모두 전송합니다.</li>
</ul>

<h3>원격 파일 바로 편집</h3>
<ul>
  <li>원격 파일을 <b>편집(열기)</b>하면 임시로 내려받아 기본 편집기로 열고, 저장하면 자동으로 다시 업로드합니다. 매번 받고 올리는 과정 없이 서버 파일을 바로 고칠 수 있습니다.</li>
</ul>

<h3>단축키</h3>
<table class="keys">
  <tr><td><kbd>Enter</kbd> / 더블클릭</td><td>폴더 들어가기 · 원격 파일 받기</td></tr>
  <tr><td><kbd>Backspace</kbd></td><td>상위 폴더</td></tr>
  <tr><td><kbd>F2</kbd></td><td>이름 변경</td></tr>
  <tr><td><kbd>Delete</kbd></td><td>삭제</td></tr>
  <tr><td><kbd>F5</kbd></td><td>새로고침</td></tr>
  <tr><td><kbd>Ctrl</kbd>/<kbd>Shift</kbd>+클릭</td><td>여러 개 선택</td></tr>
  <tr><td><kbd>Esc</kbd></td><td>열린 창·메뉴 닫기</td></tr>
</table>

<h3>보안</h3>
<ul>
  <li>저장한 비밀번호는 Windows 계정에 묶인 방식(DPAPI)으로 <b>암호화</b>되어, 이 PC의 이 사용자만 풀 수 있습니다. iDOT와 같은 방식이라 iDOT에서 가져온 비밀번호도 그대로 쓸 수 있습니다.</li>
  <li><b>SFTP</b>는 SSH 위에서 암호화되어 전송됩니다. <b>FTP</b>는 평문이므로 가능하면 SFTP를 권장합니다.</li>
</ul>
`;

const HELP_EN = `
<h3>Connecting</h3>
<ul>
  <li><b>Quick connect</b>: in the top bar, choose the protocol (SFTP/FTP) and enter host, port, user and password, then press <b>Quick connect</b> or <kbd>Enter</kbd>. While connected, the same button becomes <b>Disconnect</b>.</li>
  <li><b>Site manager</b>: open it with the <b>Sites</b> button. Save servers and organize them into <b>groups</b>; <b>double-click</b> a site or press <b>Connect</b> to open it. Drag to reorder or move into a group; drop on an empty area to remove it from its group.</li>
  <li>Hover an item for <b>rename (✎), duplicate (⧉) and delete (✕)</b>. Duplicating a group also duplicates its sites.</li>
  <li><b>Import from iDOT</b>: brings in this PC's iDOT connection list as-is (groups and passwords included). <b>Import file</b> also reads iDrop/iDOT CSV and Xshell sessions (<code>.xsh</code>, <code>.xts</code>). Existing sites are skipped.</li>
  <li><b>File name encoding</b>: choose <b>Auto</b>, UTF-8 or EUC-KR for Korean file names on EUC-KR servers. Auto is correct in most cases.</li>
</ul>

<h3>Browsing</h3>
<ul>
  <li>The left panel is <b>Local</b> (your PC), the right is <b>Remote</b> (the server). <b>Double-click</b> to enter a folder, type a path to jump there, and <kbd>Backspace</kbd> to go up.</li>
  <li>Switch disks with the <b>drive</b> selector on the local panel. Each panel has <b>📁+</b> new folder, <b>👁</b> show hidden files and <b>⟳</b> (or <kbd>F5</kbd>) refresh.</li>
  <li>Click a column header (name, size, date) to <b>sort</b>; click again to toggle ascending/descending.</li>
  <li><kbd>Ctrl</kbd>/<kbd>Shift</kbd>+click to select <b>multiple</b> items.</li>
</ul>

<h3>Transfer</h3>
<ul>
  <li><b>Drag files or folders from one panel to the other</b> to transfer them: remote→local is a <b>download</b>, local→remote is an <b>upload</b>. You can also <b>double-click</b> a remote file to download it.</li>
  <li>Transfers queue up in the <b>transfer queue</b> below and run in order with progress. The buttons at the queue header <b>cancel (✕), retry (↻) and clear finished (🧹)</b>.</li>
  <li>If a file with the same name exists you are asked to <b>Overwrite / Rename / Skip</b>, with <b>Apply to all</b> for the rest.</li>
  <li>Uploading or downloading a folder transfers all of its subfolders and files.</li>
</ul>

<h3>Edit remote files in place</h3>
<ul>
  <li><b>Edit (open)</b> a remote file to download it temporarily, open it in your default editor, and re-upload automatically when you save — edit server files directly without downloading and uploading each time.</li>
</ul>

<h3>Shortcuts</h3>
<table class="keys">
  <tr><td><kbd>Enter</kbd> / double-click</td><td>Enter a folder · download a remote file</td></tr>
  <tr><td><kbd>Backspace</kbd></td><td>Parent folder</td></tr>
  <tr><td><kbd>F2</kbd></td><td>Rename</td></tr>
  <tr><td><kbd>Delete</kbd></td><td>Delete</td></tr>
  <tr><td><kbd>F5</kbd></td><td>Refresh</td></tr>
  <tr><td><kbd>Ctrl</kbd>/<kbd>Shift</kbd>+click</td><td>Select multiple</td></tr>
  <tr><td><kbd>Esc</kbd></td><td>Close the open dialog or menu</td></tr>
</table>

<h3>Security</h3>
<ul>
  <li>Saved passwords are <b>encrypted</b> with your Windows account (DPAPI), so only you on this PC can decrypt them. It is the same scheme as iDOT, so passwords imported from iDOT work directly.</li>
  <li><b>SFTP</b> is encrypted over SSH. <b>FTP</b> is plaintext, so prefer SFTP when possible.</li>
</ul>
`;

export function helpHtml(): string {
  return lang === "en" ? NOTICE_EN + HELP_EN : NOTICE_KO + HELP_KO;
}
