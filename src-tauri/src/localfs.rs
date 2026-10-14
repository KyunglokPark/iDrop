// 로컬 파일 패널 — 나열/드라이브/조작.
// 삭제는 영구 삭제(휴지통 미경유)이므로 프런트가 확인 대화상자를 띄운다.

use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use crate::tr_s;

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LocalEntry {
    pub name: String,
    pub is_dir: bool,
    pub size: u64,
    /// epoch 초 (0 = 알 수 없음)
    pub mtime: u64,
    pub hidden: bool,
    pub readonly: bool,
}

#[cfg(windows)]
#[link(name = "kernel32")]
extern "system" {
    fn GetLogicalDrives() -> u32;
}

/// 존재하는 드라이브 목록 ("C:\\", "D:\\", ...). 비트마스크라 이동식 매체를 건드리지 않는다.
#[tauri::command]
pub fn local_drives() -> Vec<String> {
    #[cfg(windows)]
    {
        let mask = unsafe { GetLogicalDrives() };
        (0..26)
            .filter(|i| mask & (1 << i) != 0)
            .map(|i| format!("{}:\\", (b'A' + i as u8) as char))
            .collect()
    }
    #[cfg(not(windows))]
    {
        vec!["/".to_string()]
    }
}

/// 시작 디렉터리 (사용자 홈)
#[tauri::command]
pub fn local_home() -> String {
    #[cfg(windows)]
    {
        std::env::var("USERPROFILE").unwrap_or_else(|_| "C:\\".into())
    }
    #[cfg(not(windows))]
    {
        std::env::var("HOME").unwrap_or_else(|_| "/".into())
    }
}

fn meta_entry(name: String, meta: &std::fs::Metadata) -> LocalEntry {
    let mtime = meta
        .modified()
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_secs())
        .unwrap_or(0);
    #[cfg(windows)]
    let (hidden, readonly) = {
        use std::os::windows::fs::MetadataExt;
        let attr = meta.file_attributes();
        (attr & 0x2 != 0, attr & 0x1 != 0)
    };
    #[cfg(not(windows))]
    let (hidden, readonly) = (name.starts_with('.'), meta.permissions().readonly());
    LocalEntry {
        name,
        is_dir: meta.is_dir(),
        size: if meta.is_dir() { 0 } else { meta.len() },
        mtime,
        hidden,
        readonly,
    }
}

/// 디렉터리 나열. (정규화된 경로, 항목들)을 돌려준다.
#[tauri::command]
pub async fn local_list(path: String) -> Result<(String, Vec<LocalEntry>), String> {
    let p = PathBuf::from(path.trim());
    let canon = tokio::fs::canonicalize(&p)
        .await
        .map_err(|e| tr_s!(format!("폴더를 열 수 없습니다: {e}"), format!("cannot open folder: {e}")))?;
    let mut rd = tokio::fs::read_dir(&canon)
        .await
        .map_err(|e| tr_s!(format!("폴더를 열 수 없습니다: {e}"), format!("cannot open folder: {e}")))?;
    let mut out = Vec::new();
    while let Ok(Some(ent)) = rd.next_entry().await {
        let name = ent.file_name().to_string_lossy().into_owned();
        // 심볼릭 링크는 대상 기준 메타데이터 (실패하면 링크 자체)
        let meta = match ent.metadata().await {
            Ok(m) => m,
            Err(_) => continue,
        };
        out.push(meta_entry(name, &meta));
    }
    // Windows의 \\?\ 접두사는 보기 흉하므로 벗긴다
    let mut shown = canon.to_string_lossy().into_owned();
    if let Some(s) = shown.strip_prefix(r"\\?\") {
        shown = s.to_string();
    }
    Ok((shown, out))
}

#[tauri::command]
pub async fn local_mkdir(path: String) -> Result<(), String> {
    tokio::fs::create_dir(Path::new(&path)).await.map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn local_rename(from: String, to: String) -> Result<(), String> {
    tokio::fs::rename(Path::new(&from), Path::new(&to)).await.map_err(|e| e.to_string())
}

/// 영구 삭제 (폴더는 재귀). 호출 전 프런트에서 확인을 받는다.
#[tauri::command]
pub async fn local_remove(paths: Vec<String>) -> Result<(), String> {
    for p in paths {
        let path = PathBuf::from(&p);
        let meta = tokio::fs::symlink_metadata(&path).await.map_err(|e| e.to_string())?;
        let r = if meta.is_dir() {
            tokio::fs::remove_dir_all(&path).await
        } else {
            tokio::fs::remove_file(&path).await
        };
        r.map_err(|e| tr_s!(format!("{p}: 삭제 실패 ({e})"), format!("{p}: delete failed ({e})")))?;
    }
    Ok(())
}
