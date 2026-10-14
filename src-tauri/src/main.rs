// Prevents additional console window on Windows in release, DO NOT REMOVE!!
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    // 파일·폴더 선택 창 도우미 프로세스 (lib.rs의 run_dialog 참고)
    let mut args = std::env::args().skip(1);
    if args.next().as_deref() == Some("--idrop-dialog") {
        idrop_lib::dialog_helper(&args.next().unwrap_or_default());
        return;
    }
    idrop_lib::run()
}
