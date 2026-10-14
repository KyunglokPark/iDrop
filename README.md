# iDrop

FileZilla를 대체하는 **SFTP 전용** 파일 전송 프로그램 (Windows). iDOT(SSH 터미널)의
자매 제품으로, 네이티브 SSH/SFTP 스택을 공유한다. 핵심 차별점은 **한국 레거시
서버의 EUC-KR 파일명 완벽 지원** — UTF-8/EUC-KR 이름이 혼재해도 깨지지 않는다.

## 구조

```
src/                프론트엔드 (TypeScript, 프레임워크 없음)
  main.ts           앱 셸 — 퀵커넥트, 상태, 이벤트 배선
  dialogs.ts        호스트 키 확인 / 인증 입력 대화상자
  sessionio.ts      세션 가져오기(iDOT CSV·Xshell·PuTTY) [iDOT에서 복사]
  i18n.ts           한/영 문자열
src-tauri/src/      백엔드 (Rust)
  connect.rs        SSH 접속·인증 — iDOT에서 추출, 프롬프트를 이벤트+대화상자로 교체
  pool.rs           사이트 연결 + SFTP 채널 풀 (탐색 1채널 + 전송 채널 풀)
  sftp.rs           SFTP v3 클라이언트 [iDOT에서 복사 + NameCodec 공유 수정]
  queue.rs          전송 큐 엔진 (M3)
  localfs.rs        로컬 파일 패널 (M2)
  hostkey.rs/win.rs/import.rs   [iDOT에서 복사]
dev/sftp-e2e/       검증 크레이트 — WSL의 실제 OpenSSH sftp-server 상대
```

### NameCodec — iDOT sftp.rs 대비 유일한 수정

파일명 인코딩 상태(euc 플래그 + 원시 바이트 캐시)를 `NameCodec`으로 분리해
**같은 사이트의 모든 SFTP 채널이 공유**한다. 탐색 채널이 목록에서 캐시한 EUC-KR
원시 바이트를 전송 채널이 그대로 쓰기 위한 것 — 채널마다 따로면 "no such file".

## 빌드 (Windows)

```powershell
cd D:\Source\IDOIT\iDrop
npm install
npm run tauri dev     # 개발 실행 (vite 포트 1430 — iDOT의 1420과 공존)
npm run tauri build   # 배포 (msi)
```

## 검증 (WSL)

WSL sshd(포트 2222)가 떠 있어야 한다. 테스트는 전용 ed25519 키를 만들어
`~/.ssh/authorized_keys`에 등록해 쓰므로 비밀번호가 필요 없다.

```bash
cd dev/sftp-e2e
cargo run --bin roundtrip   # 업/다운로드 해시 왕복 + rename + 재귀 삭제
cargo run --bin euckr       # EUC-KR/UTF-8 혼재 + 채널 간 NameCodec 공유 게이트
```

e2e는 `#[path]`로 실제 `src-tauri/src/sftp.rs`를 그대로 포함한다 (복사본 드리프트 없음).
