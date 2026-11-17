//! 用一个假的 `vicinae` 脚本代替真的：按场景输出、退出，并记下参数和 stdin。

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

use kernel_api::ErrorKind;
use prompt_contract::testing::{Scenario, prompt_contract};
use serde_json::json;

use super::*;

/// 写一个假 vicinae：把参数逐行写进 `<脚本>.args`、stdin 写进 `<脚本>.stdin`，然后执行 `body`。
fn fake(dir: &Path, name: &str, body: &str) -> PathBuf {
    use std::os::unix::fs::PermissionsExt;
    let path = dir.join(name);
    let script =
        format!("#!/bin/sh\nprintf '%s\\n' \"$@\" > \"$0.args\"\ncat > \"$0.stdin\"\n{body}\n");
    std::fs::write(&path, script).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    path
}

fn vicinae(command: &Path) -> Vicinae {
    Vicinae(Config {
        command: command.to_string_lossy().into_owned(),
        ..Config::default()
    })
}

fn body(scenario: &Scenario) -> String {
    match scenario {
        // 真的 vicinae 输出时末尾带换行
        Scenario::Types(text) => format!("printf '%s\\n' '{text}'"),
        Scenario::EntersNothing => "exit 0".to_owned(),
        Scenario::Escapes => "exit 1".to_owned(),
    }
}

#[tokio::test]
async fn passes_the_prompt_contract() {
    let dir = tempfile::tempdir().unwrap();
    let n = AtomicUsize::new(0);
    prompt_contract(|scenario| {
        let name = format!("vicinae-{}", n.fetch_add(1, Ordering::Relaxed));
        Arc::new(vicinae(&fake(dir.path(), &name, &body(&scenario)))) as Arc<dyn Prompt>
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn arguments_and_an_empty_list() {
    let dir = tempfile::tempdir().unwrap();
    let script = fake(dir.path(), "vicinae", "printf 'x\\n'");
    let prompt = Vicinae(Config {
        command: script.to_string_lossy().into_owned(),
        width: Some(700),
        height: None,
    });
    prompt
        .ask(PromptRequest {
            placeholder: "记点什么… #t 待办".to_owned(),
            initial: "#t ".to_owned(),
        })
        .await
        .unwrap();
    let args = std::fs::read_to_string(dir.path().join("vicinae.args")).unwrap();
    assert_eq!(
        args.lines().collect::<Vec<_>>(),
        [
            "dmenu",
            "--placeholder",
            "记点什么… #t 待办",
            "--no-section",
            "--no-quick-look",
            "--query",
            "#t ",
            "--width",
            "700"
        ]
    );
    let stdin = std::fs::read(dir.path().join("vicinae.stdin")).unwrap();
    assert!(stdin.is_empty(), "列表必须是空的，否则空回车会选中第一项");
}

#[tokio::test]
async fn no_initial_text_means_no_query_argument() {
    let dir = tempfile::tempdir().unwrap();
    let script = fake(dir.path(), "vicinae", "exit 1");
    vicinae(&script)
        .ask(PromptRequest::default())
        .await
        .unwrap();
    let args = std::fs::read_to_string(dir.path().join("vicinae.args")).unwrap();
    assert!(!args.lines().any(|a| a == "--query"), "{args}");
}

#[tokio::test]
async fn a_crash_or_a_missing_program_is_unavailable() {
    let dir = tempfile::tempdir().unwrap();
    let script = fake(dir.path(), "vicinae", "echo 'no display' >&2; exit 3");
    let err = vicinae(&script)
        .ask(PromptRequest::default())
        .await
        .unwrap_err();
    assert_eq!(err.kind, ErrorKind::Unavailable);
    assert!(
        err.message.contains("退出码 3") && err.message.contains("no display"),
        "{err}"
    );

    let err = vicinae(&dir.path().join("not-there"))
        .ask(PromptRequest::default())
        .await
        .unwrap_err();
    assert_eq!(err.kind, ErrorKind::Unavailable, "{err}");
}

#[tokio::test]
async fn config_is_checked_at_startup() {
    let err = testkit::app()
        .with(PromptVicinae)
        .config("prompt-vicinae", json!({ "command": " " }))
        .unwrap()
        .boot()
        .await
        .err()
        .unwrap();
    assert_eq!(err.kind, ErrorKind::Config);
    let err = testkit::app()
        .with(PromptVicinae)
        .config("prompt-vicinae", json!({ "colour": "red" }))
        .unwrap()
        .boot()
        .await
        .err()
        .unwrap();
    assert_eq!(err.kind, ErrorKind::Config);
}
