// 원격 파일 바로 편집 (FileZilla의 View/Edit).
//
// 원격 파일을 임시 폴더로 받아 OS 기본 에디터로 열고, 백그라운드 감시 태스크가
// 저장(파일 변경)을 감지하면 자동으로 원격에 다시 올린다. 감시는 사이트가 종료되면
// (연결 끊기/사이트 교체) 멈춘다. 재업로드는 pool의 전송 연결을 재사용한다.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use tauri::{AppHandle, Emitter};

use crate::pool::SitePool;
use crate::{sftp, win};

static NEXT: AtomicU64 = AtomicU64::new(1);

fn edit_root() -> PathBuf {
    std::env::temp_dir().join("iDrop-edit")
}

/// 원격 파일을 받아 기본 에디터로 열고 변경 감시를 시작한다.
pub async fn start(app: AppHandle, site: Arc<SitePool>, remote: String) -> Result<(), String> {
    let name = sftp::base(&remote).to_string();
    // 고유 하위 폴더에 원본 파일명 그대로 둔다 (에디터 제목·저장이 원래 이름으로 되게).
    let id = NEXT.fetch_add(1, Ordering::Relaxed);
    let dir = edit_root().join(id.to_string());
    tokio::fs::create_dir_all(&dir).await.map_err(|e| e.to_string())?;
    let local = dir.join(&name);

    // 1) 다운로드
    let s = site.take_xfer().await?;
    let no_cancel = AtomicBool::new(false);
    let res = s.download_file(&remote, &local, 0, &no_cancel, &mut |_| {}).await;
    site.give_back(s);
    res?;

    // 2) 기본 에디터로 열기
    win::shell_open(&local.to_string_lossy())?;
    emit(&app, "opened", &name, None);

    // 3) 변경 감시 → 자동 재업로드
    watch(app, site, local, remote, name);
    Ok(())
}

fn watch(app: AppHandle, site: Arc<SitePool>, local: PathBuf, remote: String, name: String) {
    tauri::async_runtime::spawn(async move {
        // 방금 받은 파일을 기준으로 '변경'을 판단한다.
        let mut uploaded = sig(&local).await;
        let mut seen = uploaded;
        loop {
            tokio::time::sleep(Duration::from_millis(1500)).await;
            if site.is_shutdown() {
                break; // 연결 종료 → 감시 중단
            }
            let cur = sig(&local).await;
            if cur.is_none() {
                break; // 임시 파일이 사라짐 (사용자 정리) → 감시 종료
            }
            // 업로드본과 다르고(변경됨) 한 주기 동안 안정적이면(직전 관측과 같음) 재업로드.
            // 에디터가 파일을 쓰는 도중(크기 변동)에 올리지 않도록 안정화 대기.
            if cur != uploaded && cur == seen {
                match site.take_xfer().await {
                    Ok(s) => {
                        let nc = AtomicBool::new(false);
                        let r = s.upload_file(&local, &remote, 0, &nc, &mut |_| {}).await;
                        site.give_back(s);
                        match r {
                            Ok(()) => {
                                uploaded = cur;
                                emit(&app, "saved", &name, None);
                            }
                            Err(e) => emit(&app, "error", &name, Some(e)),
                        }
                    }
                    Err(e) => emit(&app, "error", &name, Some(e)),
                }
            }
            seen = cur;
        }
    });
}

/// 파일 서명 (mtime 나노초, 크기) — 변경 감지용.
async fn sig(p: &Path) -> Option<(u128, u64)> {
    let m = tokio::fs::metadata(p).await.ok()?;
    let mtime = m
        .modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    Some((mtime, m.len()))
}

fn emit(app: &AppHandle, state: &str, name: &str, error: Option<String>) {
    #[derive(serde::Serialize, Clone)]
    #[serde(rename_all = "camelCase")]
    struct E {
        state: String, // "opened" | "saved" | "error"
        name: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        error: Option<String>,
    }
    let _ = app.emit("edit-state", E { state: state.into(), name: name.into(), error });
}
