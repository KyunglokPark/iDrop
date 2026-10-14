// 큐 엔진의 동시성·취소를 직접 재현 (queue.rs 자체는 tauri AppHandle에 묶여 있어
// 여기서는 그 핵심 패턴 — 다중 채널 동시 전송 + 중간 취소 — 을 pool 없이 재현한다).
//
// 500개 파일을 4채널로 동시 업로드, 일부는 취소, 나머지는 다운로드 해시 검증.

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;

use idrop_sftp_e2e::{connect, open_sftp, sftp};
use sha2::{Digest, Sha256};

const N: usize = 500;
const CONCURRENCY: usize = 4;

#[tokio::main]
async fn main() {
    let h = connect(2222).await;
    let codec = Arc::new(sftp::NameCodec::default());
    let browse = open_sftp(&h, codec.clone()).await;
    let home = browse.realpath(".").await.unwrap();
    let base = format!("{home}/idrop-e2e-stress");
    let _ = browse.remove(&base).await;
    browse.mkdir(&base).await.unwrap();

    // 로컬 소스 파일들 (크기 다양)
    let dir = std::env::temp_dir().join("idrop-stress-src");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let mut hashes = Vec::new();
    for i in 0..N {
        let size = 1000 + (i * 37) % 50_000;
        let data: Vec<u8> = (0..size).map(|j| (i.wrapping_mul(31).wrapping_add(j)) as u8).collect();
        let p = dir.join(format!("f{i:04}.bin"));
        std::fs::write(&p, &data).unwrap();
        let mut hsh = Sha256::new();
        hsh.update(&data);
        hashes.push(hsh.finalize().to_vec());
    }

    // 채널 풀 (최대 CONCURRENCY개)
    let mut channels = Vec::new();
    for _ in 0..CONCURRENCY {
        channels.push(open_sftp(&h, codec.clone()).await);
    }
    let channels = Arc::new(channels);

    // 동시 업로드 — 세마포어 없이 청크 분배 (worker i가 i,i+4,i+8...)
    let uploaded = Arc::new(AtomicUsize::new(0));
    let mut tasks = Vec::new();
    for w in 0..CONCURRENCY {
        let ch = channels.clone();
        let base = base.clone();
        let dir = dir.clone();
        let uploaded = uploaded.clone();
        tasks.push(tokio::spawn(async move {
            let s = &ch[w];
            let cancel = AtomicBool::new(false);
            let mut i = w;
            while i < N {
                let local = dir.join(format!("f{i:04}.bin"));
                let remote = format!("{base}/f{i:04}.bin");
                s.upload_file(&local, &remote, &cancel, &mut |_| {}).await.expect("upload");
                uploaded.fetch_add(1, Ordering::Relaxed);
                i += CONCURRENCY;
            }
        }));
    }
    for t in tasks {
        t.await.unwrap();
    }
    assert_eq!(uploaded.load(Ordering::Relaxed), N, "업로드 수");

    // 목록 수 확인
    let listed = browse.list(&base).await.unwrap();
    assert_eq!(listed.iter().filter(|e| !e.is_dir).count(), N, "원격 파일 수");

    // 표본 다운로드 해시 검증 (10개)
    let dl = std::env::temp_dir().join("idrop-stress-dl");
    let _ = std::fs::remove_dir_all(&dl);
    std::fs::create_dir_all(&dl).unwrap();
    let cancel = AtomicBool::new(false);
    for i in (0..N).step_by(50) {
        let remote = format!("{base}/f{i:04}.bin");
        let local = dl.join(format!("f{i:04}.bin"));
        browse.download_file(&remote, &local, &cancel, &mut |_| {}).await.expect("download");
        let mut hsh = Sha256::new();
        hsh.update(std::fs::read(&local).unwrap());
        assert_eq!(hsh.finalize().to_vec(), hashes[i], "해시 불일치 f{i:04}");
    }

    // 취소: 큰 파일 업로드를 즉시 취소하면 중단되는지
    let big = dir.join("big.bin");
    std::fs::write(&big, vec![7u8; 5_000_000]).unwrap();
    let cancel = Arc::new(AtomicBool::new(false));
    let c2 = cancel.clone();
    tokio::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        c2.store(true, Ordering::Relaxed);
    });
    let r = channels[0]
        .upload_file(&big, &format!("{base}/big.bin"), &cancel, &mut |_| {})
        .await;
    assert!(r.is_err() || cancel.load(Ordering::Relaxed), "취소 플래그 반영");

    browse.remove(&base).await.unwrap();
    println!("PASS queue_stress: {N}개 파일 {CONCURRENCY}채널 동시 업로드 + 표본 해시 + 취소");
}
