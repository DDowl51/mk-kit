//! 假的 niri、wl-paste、wl-copy：都是 shell 脚本，把收到的东西记在目录里。
//!
//! 这些测试一个一个跑（[`SERIAL`]）：一个测试正在写脚本时，另一个测试 fork 出的子进程会带着这个
//! 写句柄，exec 脚本就会报「Text file busy」。

use std::path::{Path, PathBuf};

use screenshot_contract::testing::screenshot_contract;

use super::*;

fn script(dir: &Path, name: &str, body: &str) -> String {
    use std::os::unix::fs::PermissionsExt;
    let path = dir.join(name);
    std::fs::write(
        &path,
        format!("#!/bin/sh\nLOG='{}'\n{body}\n", dir.display()),
    )
    .unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    path.to_string_lossy().into_owned()
}

/// 只有窗口 7。截图时写一个 PNG，并且像真的 niri 一样把剪贴板改掉。
const NIRI: &str = r#"
case "$*" in
  "msg -j windows") echo '[{"id":7,"title":"x"}]' ;;
  "msg action screenshot-window"*)
    while [ $# -gt 0 ]; do [ "$1" = --path ] && p="$2"; shift; done
    printf '\211PNG\r\n\032\nfake' > "$p"
    echo niri-screenshot > "$LOG/clipboard" ;;
  *) exit 2 ;;
esac"#;

static SERIAL: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

struct Rig {
    dir: tempfile::TempDir,
    niri: Niri,
    _serial: tokio::sync::MutexGuard<'static, ()>,
}

impl Rig {
    async fn new(clipboard: Option<&str>) -> Self {
        let serial = SERIAL.lock().await;
        let dir = tempfile::tempdir().unwrap();
        let d = dir.path();
        if let Some(text) = clipboard {
            std::fs::write(d.join("clipboard"), text).unwrap();
        }
        let wl_paste = script(
            d,
            "wl-paste",
            r#"[ -f "$LOG/clipboard" ] || { echo 'Nothing is copied' >&2; exit 1; }
case "$1" in
  --list-types) printf 'text/plain;charset=utf-8\ntext/plain\n' ;;
  *) echo "$*" >> "$LOG/paste-args"; cat "$LOG/clipboard" ;;
esac"#,
        );
        let wl_copy = script(
            d,
            "wl-copy",
            r#"echo "$*" > "$LOG/copy-args"; if [ "$1" = --clear ]; then rm -f "$LOG/clipboard"; else cat > "$LOG/clipboard"; fi"#,
        );
        let niri = script(d, "niri", NIRI);
        let config = Config {
            niri,
            wl_paste,
            wl_copy,
            timeout_ms: 1_000,
            ..Config::default()
        };
        Self {
            niri: Niri(config),
            dir,
            _serial: serial,
        }
    }

    fn file(&self, name: &str) -> PathBuf {
        self.dir.path().join(name)
    }

    fn read(&self, name: &str) -> String {
        std::fs::read_to_string(self.file(name)).unwrap_or_default()
    }
}

#[tokio::test]
async fn passes_the_screenshot_contract() {
    let rig = Rig::new(Some("原来的剪贴板")).await;
    let out = rig.file("out");
    std::fs::create_dir_all(&out).unwrap();
    screenshot_contract(
        Arc::new(Niri(rig.niri.0.clone())),
        7,
        8,
        &out.to_string_lossy(),
    )
    .await
    .unwrap();
}

#[tokio::test]
async fn the_clipboard_is_put_back() {
    let rig = Rig::new(Some("原来的剪贴板")).await;
    let path = rig.file("shot.png").to_string_lossy().into_owned();
    rig.niri.window(7, path).await.unwrap();
    assert_eq!(rig.read("clipboard"), "原来的剪贴板");
    assert_eq!(
        rig.read("copy-args").trim(),
        "--type text/plain;charset=utf-8"
    );
    assert_eq!(
        rig.read("paste-args").trim(),
        "--no-newline --type text/plain;charset=utf-8"
    );
}

#[tokio::test]
async fn an_empty_clipboard_stays_empty() {
    let rig = Rig::new(None).await;
    let path = rig.file("shot.png").to_string_lossy().into_owned();
    rig.niri.window(7, path).await.unwrap();
    assert!(!rig.file("clipboard").exists(), "截图留在剪贴板里了");
    assert_eq!(rig.read("copy-args").trim(), "--clear");
}

#[tokio::test]
async fn restoring_can_be_turned_off() {
    let mut rig = Rig::new(Some("原来的剪贴板")).await;
    rig.niri.0.restore_clipboard = false;
    let path = rig.file("shot.png").to_string_lossy().into_owned();
    rig.niri.window(7, path).await.unwrap();
    assert_eq!(rig.read("clipboard").trim(), "niri-screenshot");
    assert!(!rig.file("copy-args").exists());
}

#[tokio::test]
async fn a_file_that_never_appears_is_unavailable() {
    let mut rig = Rig::new(None).await;
    let lazy = script(
        rig.dir.path(),
        "lazy-niri",
        r#"[ "$*" = "msg -j windows" ] && echo '[{"id":7}]'; exit 0"#,
    );
    rig.niri.0.niri = lazy;
    rig.niri.0.timeout_ms = 100;
    let path = rig.file("shot.png").to_string_lossy().into_owned();
    let err = rig.niri.window(7, path).await.unwrap_err();
    assert_eq!(err.kind, kernel_api::ErrorKind::Unavailable);
    assert!(err.message.contains("没等到"), "{err}");
}

#[tokio::test]
async fn niri_not_running_is_unavailable() {
    let mut rig = Rig::new(None).await;
    rig.niri.0.niri = script(rig.dir.path(), "dead-niri", "echo 'no socket' >&2; exit 1");
    let path = rig.file("shot.png").to_string_lossy().into_owned();
    let err = rig.niri.window(7, path).await.unwrap_err();
    assert_eq!(err.kind, kernel_api::ErrorKind::Unavailable);
    assert!(err.message.contains("no socket"), "{err}");
}
