// 전송 큐 엔진 — Rust가 단일 진실 원천, 프런트는 이벤트로 미러링.
//
// 흐름: queue_add → 나열 태스크가 재귀로 펼쳐 항목 생성(queue-add 배치 이벤트)
//   → 스케줄러가 동시성만큼 워커 스폰 → 워커가 채널 풀에서 전송 → queue-state
//   진행률은 워커 공용 누적 + 150ms 단일 집계(queue-progress).
// 충돌 처리/재시도는 M5. 지금은 덮어쓰기(upload=TRUNC) + unique(download)로 동작.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use tauri::{AppHandle, Emitter};
use tokio::sync::{oneshot, Notify};

use crate::pool::SitePool;
use crate::sftp;

/// 같은 이름이 이미 있을 때의 처리.
#[derive(Clone, Copy, PartialEq)]
pub enum Conflict {
    Ask,
    Overwrite,
    Skip,
    Rename,
}

fn parse_conflict(s: &str) -> Conflict {
    match s {
        "overwrite" => Conflict::Overwrite,
        "skip" => Conflict::Skip,
        "rename" => Conflict::Rename,
        _ => Conflict::Ask,
    }
}

#[derive(Clone, Copy, PartialEq, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Dir {
    Upload,
    Download,
}

#[derive(Clone, PartialEq, serde::Serialize)]
#[serde(rename_all = "camelCase", tag = "state", content = "error")]
pub enum ItemState {
    Queued,
    Active,
    Done,
    Failed(String),
    Cancelled,
}

struct Item {
    dir: Dir,
    local: PathBuf,
    remote: String,
    /// 전송 대상이 디렉터리 생성이면 true (파일 바이트 없음)
    is_mkdir: bool,
    state: ItemState,
    cancel: Arc<AtomicBool>,
}

#[derive(Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct ItemView {
    id: u64,
    dir: Dir,
    name: String,
    size: u64,
}

pub struct Queue {
    app: AppHandle,
    order: Mutex<Vec<u64>>,
    items: Mutex<HashMap<u64, Item>>,
    next_id: AtomicU64,
    concurrency: AtomicUsize,
    running: AtomicUsize,
    /// 집계용 누적 바이트 (id → 이번 틱까지 전송량)
    progress: Mutex<HashMap<u64, u64>>,
    done_bytes: AtomicU64,
    total_bytes: AtomicU64,
    wake: Notify,
    /// 배치 단위 "모두 적용" 정책 (Ask면 항목마다 묻는다)
    policy: Mutex<Conflict>,
    /// 충돌 답 대기 (req_id → 보낸이)
    conflict_waiters: Mutex<HashMap<u64, oneshot::Sender<Conflict>>>,
}

impl Queue {
    pub fn new(app: AppHandle) -> Arc<Queue> {
        let q = Arc::new(Queue {
            app,
            order: Mutex::new(Vec::new()),
            items: Mutex::new(HashMap::new()),
            next_id: AtomicU64::new(1),
            concurrency: AtomicUsize::new(2),
            running: AtomicUsize::new(0),
            progress: Mutex::new(HashMap::new()),
            done_bytes: AtomicU64::new(0),
            total_bytes: AtomicU64::new(0),
            wake: Notify::new(),
            policy: Mutex::new(Conflict::Ask),
            conflict_waiters: Mutex::new(HashMap::new()),
        });
        q.clone().spawn_progress_emitter();
        q
    }

    pub fn set_concurrency(&self, n: usize) {
        self.concurrency.store(n.clamp(1, 8), Ordering::Relaxed);
        self.wake.notify_one();
    }

    fn emit_state(&self, id: u64, state: &ItemState) {
        let _ = self.app.emit("queue-state", {
            #[derive(serde::Serialize, Clone)]
            struct E {
                id: u64,
                #[serde(flatten)]
                state: ItemState,
            }
            E { id, state: state.clone() }
        });
    }

    /// 150ms마다 전체 진행률을 1건으로 발행 (항목별 이벤트 난사 방지)
    fn spawn_progress_emitter(self: Arc<Self>) {
        tauri::async_runtime::spawn(async move {
            loop {
                tokio::time::sleep(Duration::from_millis(150)).await;
                let active: Vec<(u64, u64)> =
                    self.progress.lock().unwrap().iter().map(|(k, v)| (*k, *v)).collect();
                if active.is_empty() && self.running.load(Ordering::Relaxed) == 0 {
                    continue;
                }
                #[derive(serde::Serialize, Clone)]
                #[serde(rename_all = "camelCase")]
                struct Prog {
                    active: Vec<(u64, u64)>,
                    done_bytes: u64,
                    total_bytes: u64,
                }
                let _ = self.app.emit(
                    "queue-progress",
                    Prog {
                        active,
                        done_bytes: self.done_bytes.load(Ordering::Relaxed),
                        total_bytes: self.total_bytes.load(Ordering::Relaxed),
                    },
                );
            }
        });
    }

    /// 항목을 큐에 넣고 queue-add 이벤트로 통지. (나열 태스크가 호출)
    fn push(&self, dir: Dir, local: PathBuf, remote: String, is_mkdir: bool, size: u64) -> u64 {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let name = if dir == Dir::Upload {
            local.file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default()
        } else {
            sftp::base(&remote).to_string()
        };
        self.items.lock().unwrap().insert(
            id,
            Item {
                dir,
                local,
                remote,
                is_mkdir,
                state: ItemState::Queued,
                cancel: Arc::new(AtomicBool::new(false)),
            },
        );
        self.order.lock().unwrap().push(id);
        self.total_bytes.fetch_add(size, Ordering::Relaxed);
        let _ = self.app.emit("queue-add", vec![ItemView { id, dir, name, size }]);
        id
    }

    /// 전송 요청: sources(로컬 또는 원격 경로)를 dest_dir 아래로.
    /// 나열(재귀 펼치기)은 별도 태스크에서 비동기로 진행한다.
    pub fn enqueue(self: &Arc<Self>, site: Arc<SitePool>, dir: Dir, sources: Vec<String>, dest_dir: String) {
        self.reset_policy(); // 새 배치는 다시 묻기부터
        let q = self.clone();
        tauri::async_runtime::spawn(async move {
            for src in sources {
                if let Err(e) = q.expand(&site, dir, &src, &dest_dir).await {
                    let _ = q.app.emit("queue-error", e);
                }
            }
            q.wake.notify_one();
        });
    }

    /// 한 소스를 재귀로 펼쳐 큐 항목을 만든다.
    async fn expand(&self, site: &Arc<SitePool>, dir: Dir, src: &str, dest_dir: &str) -> Result<(), String> {
        match dir {
            Dir::Upload => {
                let p = PathBuf::from(src);
                let name = p.file_name().map(|s| s.to_string_lossy().into_owned()).ok_or("bad path")?;
                let meta = tokio::fs::metadata(&p).await.map_err(|e| e.to_string())?;
                let remote = sftp::join(dest_dir, &name);
                if meta.is_dir() {
                    self.push(dir, p.clone(), remote.clone(), true, 0); // mkdir
                    let mut stack = vec![(p, remote)];
                    while let Some((lp, rp)) = stack.pop() {
                        let mut rd = tokio::fs::read_dir(&lp).await.map_err(|e| e.to_string())?;
                        while let Ok(Some(ent)) = rd.next_entry().await {
                            let cn = ent.file_name().to_string_lossy().into_owned();
                            let m = match ent.metadata().await {
                                Ok(m) => m,
                                Err(_) => continue,
                            };
                            let cr = sftp::join(&rp, &cn);
                            if m.is_dir() {
                                self.push(dir, ent.path(), cr.clone(), true, 0);
                                stack.push((ent.path(), cr));
                            } else {
                                self.push(dir, ent.path(), cr, false, m.len());
                            }
                        }
                    }
                } else {
                    self.push(dir, p, remote, false, meta.len());
                }
            }
            Dir::Download => {
                let s = site.browse().await?;
                let name = sftp::base(src).to_string();
                let attrs = s.stat(src, true).await?;
                let local_root = PathBuf::from(dest_dir).join(&name);
                if attrs.is_dir() {
                    self.push(dir, local_root.clone(), src.to_string(), true, 0); // mkdir local
                    let mut stack = vec![(src.to_string(), local_root)];
                    while let Some((rp, lp)) = stack.pop() {
                        for ent in s.list(&rp).await? {
                            let cr = sftp::join(&rp, &ent.name);
                            let cl = lp.join(&ent.name);
                            if ent.is_dir {
                                self.push(dir, cl.clone(), cr.clone(), true, 0);
                                stack.push((cr, cl));
                            } else {
                                self.push(dir, cl, cr, false, ent.size);
                            }
                        }
                    }
                } else {
                    self.push(dir, local_root, src.to_string(), false, attrs.size.unwrap_or(0));
                }
            }
        }
        Ok(())
    }

    /// 스케줄러 — 빈 슬롯만큼 워커를 띄운다. (명령 핸들러가 site와 함께 기동)
    pub fn start_scheduler(self: &Arc<Self>, site: Arc<SitePool>) {
        let q = self.clone();
        tauri::async_runtime::spawn(async move {
            loop {
                // 다음 Queued 항목을 동시성 범위에서 꺼낸다
                let next = {
                    let max = q.concurrency.load(Ordering::Relaxed);
                    if q.running.load(Ordering::Relaxed) >= max {
                        None
                    } else {
                        let order = q.order.lock().unwrap();
                        let mut items = q.items.lock().unwrap();
                        order
                            .iter()
                            .find(|id| matches!(items.get(id).map(|i| &i.state), Some(ItemState::Queued)))
                            .copied()
                            .inspect(|id| {
                                if let Some(it) = items.get_mut(id) {
                                    it.state = ItemState::Active;
                                }
                            })
                    }
                };
                match next {
                    Some(id) => {
                        q.running.fetch_add(1, Ordering::Relaxed);
                        q.emit_state(id, &ItemState::Active);
                        let qc = q.clone();
                        let sc = site.clone();
                        tauri::async_runtime::spawn(async move {
                            qc.run_item(sc, id).await;
                            qc.running.fetch_sub(1, Ordering::Relaxed);
                            qc.wake.notify_one();
                        });
                    }
                    None => {
                        if !site.is_alive() {
                            break;
                        }
                        q.wake.notified().await;
                    }
                }
            }
        });
    }

    async fn run_item(&self, site: Arc<SitePool>, id: u64) {
        // 항목 정보 복제
        let (dir, local, remote, is_mkdir, cancel) = {
            let items = self.items.lock().unwrap();
            let Some(it) = items.get(&id) else { return };
            (it.dir, it.local.clone(), it.remote.clone(), it.is_mkdir, it.cancel.clone())
        };

        let result: Result<(), String> = async {
            if is_mkdir {
                match dir {
                    Dir::Upload => {
                        let s = site.take_xfer().await?;
                        let r = s.mkdir(&remote).await;
                        site.give_back(s);
                        // 이미 있으면 성공으로 간주
                        if let Err(e) = r {
                            if !e.contains("failure") && !e.contains("exist") {
                                return Err(e);
                            }
                        }
                    }
                    Dir::Download => {
                        let _ = tokio::fs::create_dir_all(&local).await;
                    }
                }
                return Ok(());
            }

            // 충돌 검사 — 대상이 이미 있으면 정책에 따른다 (대상 경로가 바뀔 수 있다)
            let (local, remote) = self.resolve_conflict(&site, dir, id, local, remote).await?;
            if remote.is_empty() {
                return Ok(()); // 건너뛰기 (Skip)
            }

            let s = site.take_xfer().await?;
            let mut acc = 0u64;
            let res = match dir {
                Dir::Upload => {
                    s.upload_file(&local, &remote, &cancel, &mut |d| {
                        acc += d;
                        *self.progress.lock().unwrap().entry(id).or_insert(0) = acc;
                        self.done_bytes.fetch_add(d, Ordering::Relaxed);
                    })
                    .await
                }
                Dir::Download => {
                    if let Some(p) = local.parent() {
                        let _ = tokio::fs::create_dir_all(p).await;
                    }
                    s.download_file(&remote, &local, &cancel, &mut |d| {
                        acc += d;
                        *self.progress.lock().unwrap().entry(id).or_insert(0) = acc;
                        self.done_bytes.fetch_add(d, Ordering::Relaxed);
                    })
                    .await
                }
            };
            site.give_back(s);
            res
        }
        .await;

        self.progress.lock().unwrap().remove(&id);
        let state = if cancel.load(Ordering::Relaxed) {
            ItemState::Cancelled
        } else {
            match result {
                Ok(()) => ItemState::Done,
                Err(e) => ItemState::Failed(e),
            }
        };
        if let Some(it) = self.items.lock().unwrap().get_mut(&id) {
            it.state = state.clone();
        }
        self.emit_state(id, &state);
    }

    /// 충돌 해소. Skip이면 remote를 ""로, Rename이면 비충돌 경로로 바꿔 돌려준다.
    async fn resolve_conflict(
        &self,
        site: &Arc<SitePool>,
        dir: Dir,
        id: u64,
        local: PathBuf,
        remote: String,
    ) -> Result<(PathBuf, String), String> {
        let exists = match dir {
            Dir::Upload => site.browse().await?.stat(&remote, true).await.is_ok(),
            Dir::Download => tokio::fs::try_exists(&local).await.unwrap_or(false),
        };
        if !exists {
            return Ok((local, remote));
        }
        let policy = *self.policy.lock().unwrap();
        let action = match policy {
            Conflict::Ask => self.ask_conflict(id, dir, &local, &remote).await,
            other => other,
        };
        match action {
            Conflict::Skip => Ok((local, String::new())),
            Conflict::Overwrite | Conflict::Ask => Ok((local, remote)),
            Conflict::Rename => match dir {
                Dir::Upload => {
                    let s = site.browse().await?;
                    let dir_path = sftp::base_dir(&remote);
                    let mut n = 2;
                    loop {
                        let cand = format!("{dir_path}/{}", sftp::numbered(sftp::base(&remote), n));
                        if s.stat(&cand, true).await.is_err() {
                            return Ok((local, cand));
                        }
                        n += 1;
                    }
                }
                Dir::Download => {
                    let parent = local.parent().map(|p| p.to_path_buf()).unwrap_or_default();
                    let name = local.file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
                    Ok((sftp::unique_local(&parent, &name), remote))
                }
            },
        }
    }

    async fn ask_conflict(&self, id: u64, dir: Dir, local: &std::path::Path, remote: &str) -> Conflict {
        let (tx, rx) = oneshot::channel();
        self.conflict_waiters.lock().unwrap().insert(id, tx);
        #[derive(serde::Serialize, Clone)]
        #[serde(rename_all = "camelCase")]
        struct C {
            id: u64,
            dir: Dir,
            name: String,
        }
        let name = if dir == Dir::Upload {
            sftp::base(remote).to_string()
        } else {
            local.file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default()
        };
        let _ = self.app.emit("queue-conflict", C { id, dir, name });
        rx.await.unwrap_or(Conflict::Skip)
    }

    /// 프런트의 충돌 답. apply_all이면 이후 항목에도 같은 정책 적용.
    pub fn conflict_answer(&self, id: u64, action: &str, apply_all: bool) {
        let a = parse_conflict(action);
        if apply_all {
            *self.policy.lock().unwrap() = a;
        }
        if let Some(tx) = self.conflict_waiters.lock().unwrap().remove(&id) {
            let _ = tx.send(a);
        }
    }

    /// 새 전송 배치 시작 시 정책을 Ask로 되돌린다 (이전 "모두 적용"이 남지 않게).
    pub fn reset_policy(&self) {
        *self.policy.lock().unwrap() = Conflict::Ask;
    }

    pub fn cancel(&self, ids: &[u64]) {
        let mut items = self.items.lock().unwrap();
        for id in ids {
            if let Some(it) = items.get_mut(id) {
                it.cancel.store(true, Ordering::Relaxed);
                if it.state == ItemState::Queued {
                    it.state = ItemState::Cancelled;
                    self.emit_state(*id, &ItemState::Cancelled);
                }
            }
        }
    }

    pub fn retry(&self, ids: &[u64]) {
        {
            let mut items = self.items.lock().unwrap();
            for id in ids {
                if let Some(it) = items.get_mut(id) {
                    if matches!(it.state, ItemState::Failed(_) | ItemState::Cancelled) {
                        it.cancel = Arc::new(AtomicBool::new(false));
                        it.state = ItemState::Queued;
                        self.emit_state(*id, &ItemState::Queued);
                    }
                }
            }
        }
        self.wake.notify_one();
    }

    pub fn clear_done(&self) {
        let mut order = self.order.lock().unwrap();
        let mut items = self.items.lock().unwrap();
        order.retain(|id| {
            let keep = !matches!(
                items.get(id).map(|i| &i.state),
                Some(ItemState::Done) | Some(ItemState::Cancelled)
            );
            if !keep {
                items.remove(id);
            }
            keep
        });
        let _ = self.app.emit("queue-cleared", ());
    }
}
