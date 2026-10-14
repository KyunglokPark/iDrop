// iDrop — SFTP 파일 전송 앱 백엔드.
// iDOT(SSH 터미널)의 네이티브 SSH/SFTP 스택을 재사용한다:
//   sftp.rs    — SFTP v3 클라이언트 (EUC-KR 파일명 지원)  [iDOT에서 복사]
//   win.rs     — DPAPI 암호화, 파일 대화상자               [iDOT에서 복사]
//   hostkey.rs — known_hosts 확인                          [iDOT에서 복사]
//   import.rs  — PuTTY/Xshell 세션 가져오기                [iDOT에서 복사]
// 신규 모듈(마일스톤 순): connect(SSH 접속·헤드리스 인증), pool(채널 풀),
// localfs(로컬 패널), queue(전송 큐 엔진).

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use tauri::{AppHandle, Emitter, Manager, State};

mod connect;
mod hostkey;
mod import;
mod localfs;
mod pool;
mod queue;
mod sftp;
mod win;

use pool::SitePool;
use queue::Queue;

// ---- UI 언어 (한/EN) ---------------------------------------------------------
static LANG_EN: AtomicBool = AtomicBool::new(false);

pub(crate) fn en() -> bool {
    LANG_EN.load(Ordering::Relaxed)
}

/// tr!(한국어, English) — 현재 언어 쪽 식을 고른다 (&str·String 모두 가능).
macro_rules! tr {
    ($ko:expr, $en:expr) => {
        if en() {
            $en
        } else {
            $ko
        }
    };
}

/// tr_s! — tr!과 같지만 항상 String을 돌려준다 (다른 모듈에서 사용).
macro_rules! tr_s {
    ($ko:expr, $en:expr) => {
        if crate::en() {
            String::from($en)
        } else {
            String::from($ko)
        }
    };
}
pub(crate) use tr_s;

#[tauri::command]
fn set_lang(lang: String) {
    LANG_EN.store(lang == "en", Ordering::Relaxed);
}

// ---- 공용 유틸 명령 (iDOT에서 이식) ------------------------------------------

/// 내보내기: 텍스트 파일을 다운로드 폴더에 저장 ("이름 (2).csv"식 중복 회피).
#[tauri::command]
fn save_to_downloads(app: AppHandle, file_name: String, contents: String) -> Result<String, String> {
    let dir = app.path().download_dir().map_err(|e| e.to_string())?;
    let safe: String = file_name
        .chars()
        .filter(|c| !matches!(*c, '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|'))
        .collect();
    let safe = if safe.trim().is_empty() { "idrop-sites.csv".to_string() } else { safe };
    let (stem, ext) = match safe.rfind('.') {
        Some(i) => (safe[..i].to_string(), safe[i..].to_string()),
        None => (safe.clone(), String::new()),
    };
    let mut path = dir.join(&safe);
    let mut n = 2;
    while path.exists() {
        path = dir.join(format!("{stem} ({n}){ext}"));
        n += 1;
    }
    std::fs::write(&path, contents.as_bytes()).map_err(|e| e.to_string())?;
    Ok(path.to_string_lossy().into_owned())
}

#[tauri::command]
fn default_download_dir(app: AppHandle) -> String {
    app.path()
        .download_dir()
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// 저장할 비밀번호 암호화 (Windows DPAPI — 이 PC의 이 사용자만 풀 수 있다)
#[tauri::command]
fn secret_protect(values: Vec<String>) -> Result<Vec<String>, String> {
    values.iter().map(|v| win::protect_str(v)).collect()
}

/// 저장된 비밀번호 풀기. 풀 수 없는 값(다른 PC·사용자에서 암호화)은 빈 문자열.
#[tauri::command]
fn secret_unprotect(values: Vec<String>) -> Vec<String> {
    values
        .iter()
        .map(|v| win::unprotect_str(v).unwrap_or_default())
        .collect()
}

#[tauri::command]
fn import_putty() -> Vec<std::collections::BTreeMap<String, String>> {
    import::putty_sessions()
}

#[derive(serde::Serialize)]
struct XshellImport {
    root: Option<String>,
    files: Vec<import::XshFile>,
}

#[tauri::command]
async fn import_xshell() -> Result<XshellImport, String> {
    tauri::async_runtime::spawn_blocking(|| {
        let (root, files) = import::xshell_sessions();
        XshellImport { root, files }
    })
    .await
    .map_err(|e| e.to_string())
}

// ---- 사이트 연결 (활성 원격 1개) ----------------------------------------------

#[derive(Default)]
struct AppState {
    site: tokio::sync::Mutex<Option<Arc<SitePool>>>,
    queue: Mutex<Option<Arc<Queue>>>,
}

fn get_queue(app: &AppHandle, state: &AppState) -> Arc<Queue> {
    let mut q = state.queue.lock().unwrap();
    q.get_or_insert_with(|| Queue::new(app.clone())).clone()
}

#[derive(Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct SiteStateEvt {
    state: String, // "connecting" | "connected" | "disconnected"
    host: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    home: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
}

fn emit_site(app: &AppHandle, state: &str, host: &str, home: Option<String>, error: Option<String>) {
    let _ = app.emit(
        "site-state",
        SiteStateEvt { state: state.into(), host: host.into(), home, error },
    );
}

/// 접속 + 인증 + 탐색 채널 준비 → 홈 디렉터리 경로를 돌려준다.
/// encoding: "auto" | "utf-8" | "euc-kr" — 새 파일 이름의 서버측 인코딩.
#[tauri::command]
async fn site_connect(
    app: AppHandle,
    state: State<'_, AppState>,
    hop: connect::Hop,
    encoding: String,
) -> Result<String, String> {
    // 이전 연결 정리
    state.site.lock().await.take();
    emit_site(&app, "connecting", &hop.host, None, None);

    let r: Result<(Arc<SitePool>, String), String> = async {
        let (handle, _user) = connect::connect_and_auth(&app, "site", &hop).await?;
        let site = SitePool::new(handle, hop.host.clone());
        match encoding.as_str() {
            "euc-kr" => site.codec.euc.store(true, Ordering::Relaxed),
            "utf-8" => site.codec.euc.store(false, Ordering::Relaxed),
            _ => {
                if let Some(euc) = connect::server_charset_is_euc(&site.handle).await {
                    site.codec.euc.store(euc, Ordering::Relaxed);
                }
            }
        }
        let home = site.browse().await?.realpath(".").await?;
        Ok((site, home))
    }
    .await;

    match r {
        Ok((site, home)) => {
            emit_site(&app, "connected", &hop.host, Some(home.clone()), None);
            // 큐 스케줄러를 이 사이트에 붙인다 (전송 워커가 채널 풀을 쓴다)
            get_queue(&app, &state).start_scheduler(site.clone());
            *state.site.lock().await = Some(site);
            Ok(home)
        }
        Err(e) => {
            emit_site(&app, "disconnected", &hop.host, None, Some(e.clone()));
            Err(e)
        }
    }
}

#[tauri::command]
async fn site_disconnect(app: AppHandle, state: State<'_, AppState>) -> Result<(), String> {
    if let Some(site) = state.site.lock().await.take() {
        emit_site(&app, "disconnected", &site.host, None, None);
    }
    Ok(())
}

async fn site_get(state: &AppState) -> Result<Arc<SitePool>, String> {
    state
        .site
        .lock()
        .await
        .clone()
        .filter(|s| s.is_alive())
        .ok_or_else(|| tr!("연결되어 있지 않습니다", "not connected").to_string())
}

/// 원격 목록 (경로 정규화 포함)
#[tauri::command]
async fn remote_list(
    state: State<'_, AppState>,
    path: String,
) -> Result<(String, Vec<sftp::Entry>), String> {
    let site = site_get(&state).await?;
    let s = site.browse().await?;
    let real = s.realpath(&path).await.unwrap_or(path);
    let list = s.list(&real).await?;
    Ok((real, list))
}

#[tauri::command]
async fn remote_mkdir(state: State<'_, AppState>, path: String) -> Result<(), String> {
    let site = site_get(&state).await?;
    site.browse().await?.mkdir(&path).await
}

#[tauri::command]
async fn remote_rename(state: State<'_, AppState>, from: String, to: String) -> Result<(), String> {
    let site = site_get(&state).await?;
    site.browse().await?.rename(&from, &to).await
}

/// 원격 삭제 (폴더는 재귀)
#[tauri::command]
async fn remote_remove(state: State<'_, AppState>, paths: Vec<String>) -> Result<(), String> {
    let site = site_get(&state).await?;
    let s = site.browse().await?;
    for p in paths {
        s.remove(&p).await?;
    }
    Ok(())
}

// ---- 전송 큐 명령 --------------------------------------------------------------

/// 전송 요청: dir="upload"면 로컬 sources → 원격 destDir, "download"면 그 반대.
#[tauri::command]
async fn queue_add(
    app: AppHandle,
    state: State<'_, AppState>,
    dir: String,
    sources: Vec<String>,
    dest_dir: String,
) -> Result<(), String> {
    let site = site_get(&state).await?;
    let q = get_queue(&app, &state);
    let d = if dir == "download" { queue::Dir::Download } else { queue::Dir::Upload };
    q.enqueue(site, d, sources, dest_dir);
    Ok(())
}

#[tauri::command]
fn queue_cancel(app: AppHandle, state: State<'_, AppState>, ids: Vec<u64>) {
    get_queue(&app, &state).cancel(&ids);
}

#[tauri::command]
fn queue_retry(app: AppHandle, state: State<'_, AppState>, ids: Vec<u64>) {
    get_queue(&app, &state).retry(&ids);
}

#[tauri::command]
fn queue_clear_done(app: AppHandle, state: State<'_, AppState>) {
    get_queue(&app, &state).clear_done();
}

#[tauri::command]
fn queue_set_concurrency(app: AppHandle, state: State<'_, AppState>, n: usize) {
    get_queue(&app, &state).set_concurrency(n);
}

#[tauri::command]
fn hostkey_answer(id: String, answer: String) {
    let a = match answer.as_str() {
        "save" => hostkey::Answer::Save,
        "once" => hostkey::Answer::Once,
        _ => hostkey::Answer::Reject,
    };
    hostkey::answer(&id, a);
}

// ---- 파일 대화상자 (도우미 프로세스 방식 — iDOT에서 이식) ---------------------
// 선택 창은 실행 파일을 "--idrop-dialog" 인자로 한 번 더 띄운 별도 프로세스에서
// 연다. 탐색기 확장 프로그램(예: 한컴오피스 HncShellExt64.dll)이 죽어도 본체는
// 살아남게 하려는 것.

#[cfg(windows)]
fn owner_hwnd(window: &tauri::WebviewWindow) -> isize {
    window.hwnd().map(|h| h.0 as isize).unwrap_or(0)
}
#[cfg(not(windows))]
fn owner_hwnd(_window: &tauri::WebviewWindow) -> isize {
    0
}

#[derive(serde::Serialize, serde::Deserialize)]
struct DialogReq {
    /// "files" | "folder"
    kind: String,
    owner: isize,
    title: String,
    #[serde(default)]
    filter: Vec<(String, String)>,
    #[serde(default)]
    multi: bool,
}

/// 도우미 프로세스 진입점 (main.rs가 "--idrop-dialog <json>"이면 호출).
pub fn dialog_helper(req_json: &str) {
    use std::io::Write;
    let out = match serde_json::from_str::<DialogReq>(req_json) {
        Ok(r) => win::on_sta_thread(move || {
            if r.kind == "folder" {
                serde_json::to_string(&win::pick_folder(r.owner, &r.title).map(|p| vec![p]).unwrap_or_default())
            } else {
                serde_json::to_string(&win::pick_files(r.owner, &r.title, &r.filter, r.multi))
            }
        })
        .unwrap_or_else(|_| "[]".into()),
        Err(_) => "[]".into(),
    };
    let mut so = std::io::stdout();
    let _ = so.write_all(out.as_bytes());
    let _ = so.flush();
}

async fn run_dialog(req: DialogReq) -> Result<Vec<String>, String> {
    let owner = req.owner;
    let res = tauri::async_runtime::spawn_blocking(move || -> Result<Vec<String>, String> {
        let exe = std::env::current_exe().map_err(|e| e.to_string())?;
        let json = serde_json::to_string(&req).map_err(|e| e.to_string())?;
        let mut cmd = std::process::Command::new(exe);
        cmd.arg("--idrop-dialog")
            .arg(json)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null());
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
        }
        win::allow_foreground();
        let out = cmd.output().map_err(|e| e.to_string())?;
        if !out.status.success() {
            return Err(tr!(
                "파일 선택 창이 비정상 종료되었습니다. 탐색기 확장 프로그램 문제일 수 있습니다. iDrop은 그대로 사용할 수 있습니다.",
                "The file dialog crashed, probably because of a shell extension. iDrop itself is unaffected."
            )
            .to_string());
        }
        serde_json::from_slice::<Vec<String>>(&out.stdout).map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())?;
    win::restore_owner(owner);
    res
}

/// 파일 열기 대화상자 (여러 개 선택 가능). filter: [["설명", "*.ext;*.ext"], ...]
#[tauri::command]
async fn pick_files(
    window: tauri::WebviewWindow,
    title: String,
    filter: Option<Vec<(String, String)>>,
    multi: Option<bool>,
) -> Result<Vec<String>, String> {
    run_dialog(DialogReq {
        kind: "files".into(),
        owner: owner_hwnd(&window),
        title,
        filter: filter.unwrap_or_default(),
        multi: multi.unwrap_or(true),
    })
    .await
}

/// 폴더 선택 대화상자 (취소하면 null)
#[tauri::command]
async fn pick_folder(window: tauri::WebviewWindow, title: String) -> Result<Option<String>, String> {
    let r = run_dialog(DialogReq {
        kind: "folder".into(),
        owner: owner_hwnd(&window),
        title,
        filter: Vec::new(),
        multi: false,
    })
    .await?;
    Ok(r.into_iter().next())
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .manage(AppState::default())
        .invoke_handler(tauri::generate_handler![
            set_lang,
            site_connect,
            site_disconnect,
            remote_list,
            remote_mkdir,
            remote_rename,
            remote_remove,
            localfs::local_drives,
            localfs::local_home,
            localfs::local_list,
            localfs::local_mkdir,
            localfs::local_rename,
            localfs::local_remove,
            queue_add,
            queue_cancel,
            queue_retry,
            queue_clear_done,
            queue_set_concurrency,
            save_to_downloads,
            default_download_dir,
            secret_protect,
            secret_unprotect,
            import_putty,
            import_xshell,
            hostkey_answer,
            connect::auth_answer,
            pick_files,
            pick_folder
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
