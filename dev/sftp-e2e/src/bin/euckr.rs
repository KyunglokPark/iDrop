// EUC-KR 파일명 + NameCodec 공유 검증 (계획서 §2.4의 게이트 테스트).
//
// 1) 서버에 "원시 EUC-KR 바이트" 이름의 파일을 만든다 (리눅스 파일명은 바이트열이라
//    UTF-8 로케일 WSL에서도 가능 — sftp enc_new가 EUC-KR 모드로 인코딩).
// 2) 탐색 채널의 list()가 그 이름을 한글로 복원하는지 확인.
// 3) ★ 다른 전송 채널(코덱 공유)이 그 경로를 열 수 있는지 — 채널마다 코덱이 따로면
//    여기서 "no such file"이 난다. UTF-8 이름과 혼재 상황도 함께.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use idrop_sftp_e2e::{connect, open_sftp, sftp};

#[tokio::main]
async fn main() {
    let h = connect(2222).await;
    let codec = Arc::new(sftp::NameCodec::default());
    let browse = open_sftp(&h, codec.clone()).await;

    let home = browse.realpath(".").await.unwrap();
    let base = format!("{home}/idrop-e2e-euckr");
    let _ = browse.remove(&base).await;
    browse.mkdir(&base).await.unwrap();

    // 1) EUC-KR 모드로 한글 이름 파일 업로드 → 서버에는 EUC-KR 바이트 이름으로 생긴다
    codec.euc.store(true, Ordering::Relaxed);
    let tmp = std::env::temp_dir().join("idrop-e2e-euckr.txt");
    std::fs::write(&tmp, "한글 내용\n").unwrap();
    let cancel = AtomicBool::new(false);
    let euc_file = format!("{base}/한글 보고서.txt");
    browse.upload_file(&tmp, &euc_file, 0, &cancel, &mut |_| {}).await.expect("euc upload");

    // UTF-8 이름 파일도 하나 (혼재 상황)
    codec.euc.store(false, Ordering::Relaxed);
    let utf_file = format!("{base}/utf8-한글.txt");
    browse.upload_file(&tmp, &utf_file, 0, &cancel, &mut |_| {}).await.expect("utf8 upload");

    // 2) 목록에서 둘 다 한글로 복원되는지
    let list = browse.list(&base).await.expect("list");
    let names: Vec<&str> = list.iter().map(|e| e.name.as_str()).collect();
    assert!(names.contains(&"한글 보고서.txt"), "EUC-KR 이름 복원 실패: {names:?}");
    assert!(names.contains(&"utf8-한글.txt"), "UTF-8 이름 복원 실패: {names:?}");

    // 서버의 원시 바이트가 실제로 다른지 확인 (EUC-KR 2바이트 vs UTF-8 3바이트)
    let euc_raw = browse.enc(&euc_file);
    let utf_raw = browse.enc(&utf_file);
    assert_ne!(euc_raw, utf_raw.clone(), "sanity");
    assert!(euc_raw.len() < format!("{base}/한글 보고서.txt").len(), "EUC-KR 바이트여야 함");

    // 3) ★ 다른 채널(코덱 공유)에서 같은 경로를 연다 — NameCodec 공유 게이트
    let xfer = open_sftp(&h, codec.clone()).await;
    let dl = std::env::temp_dir().join("idrop-e2e-euckr-dl.txt");
    let _ = std::fs::remove_file(&dl);
    xfer.download_file(&euc_file, &dl, 0, &cancel, &mut |_| {})
        .await
        .expect("★ 공유 코덱 실패 — 다른 채널에서 EUC-KR 파일을 못 열었다");
    assert_eq!(std::fs::read(&dl).unwrap(), "한글 내용\n".as_bytes());

    // 전송 채널에서 이름 변경(한글→한글)도 확인
    xfer.rename(&euc_file, &format!("{base}/한글 보고서 v2.txt")).await.expect("rename");

    browse.remove(&base).await.unwrap();
    println!("PASS euckr: EUC-KR/UTF-8 혼재 복원 + 채널 간 NameCodec 공유 + 한글 rename");
}
