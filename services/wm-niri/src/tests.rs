use std::sync::Mutex;

use clock_contract::testing::FakeClock;
use kernel_api::ErrorKind;
use log_contract::testing::MemoryLog;
use serde_json::json;
use testkit::TestApp;
use tokio::net::UnixListener;
use window_contract::WindowInfo;

use super::replay::NiriReplay;
use super::*;

/// 记下收到的焦点变化。
#[derive(Clone, Default)]
struct Seen(Arc<Mutex<Vec<FocusChanged>>>);

impl Seen {
    fn titles(&self) -> Vec<Option<String>> {
        self.0
            .lock()
            .unwrap()
            .iter()
            .map(|e| e.window.as_ref().and_then(|w| w.title.clone()))
            .collect()
    }
}

impl Component for Seen {
    fn manifest(&self) -> Manifest {
        Manifest {
            id: "seen",
            kind: Kind::Feature,
            summary: "测试用",
            provides: vec![],
            requires: vec![WINDOWS.id()],
            optional_requires: vec![],
        }
    }

    fn start(&self, _: &ConfigSection, host: &mut Host<'_>) -> Result<(), AppError> {
        let seen = self.0.clone();
        host.subscribe(&FOCUS_CHANGED, "note", move |e: FocusChanged| {
            let seen = seen.clone();
            async move {
                seen.lock().unwrap().push(e);
                Ok(())
            }
        })
    }
}

const SNAPSHOT: &str = r#"{"WindowsChanged":{"windows":[{"id":1,"title":"nvim","app_id":"ghostty","is_focused":true},{"id":2,"title":"Rust","app_id":"zen","is_focused":false}]}}"#;

fn at(ms: i64) -> Moment {
    Moment {
        utc_ms: ms,
        offset_min: 480,
    }
}

// ---------- 回放 ----------

#[tokio::test]
async fn replay_publishes_focus_changes() {
    let (replay, seen) = (NiriReplay::default(), Seen::default());
    let app = testkit::app()
        .with(replay.clone())
        .with(seen.clone())
        .boot()
        .await
        .unwrap();
    let windows = app.resolve(&WINDOWS).unwrap();
    assert_eq!(windows.focused().await.unwrap(), None);

    assert!(replay.feed(at(1_000), SNAPSHOT).await.unwrap());
    assert!(
        !replay
            .feed(at(1_500), r#"{"ConfigLoaded":{"failed":false}}"#)
            .await
            .unwrap()
    );
    assert!(
        replay
            .feed(at(2_000), r#"{"WindowFocusChanged":{"id":2}}"#)
            .await
            .unwrap()
    );
    replay.disconnect(at(3_000)).await.unwrap();
    app.settle().await;

    let events = seen.0.lock().unwrap().clone();
    assert_eq!(
        events,
        [
            FocusChanged {
                at: at(1_000),
                window: Some(WindowInfo {
                    id: 1,
                    app_id: Some("ghostty".to_owned()),
                    title: Some("nvim".to_owned()),
                    workspace: None,
                    pid: None,
                })
            },
            FocusChanged {
                at: at(2_000),
                window: Some(WindowInfo {
                    id: 2,
                    app_id: Some("zen".to_owned()),
                    title: Some("Rust".to_owned()),
                    workspace: None,
                    pid: None,
                })
            },
            FocusChanged {
                at: at(3_000),
                window: None
            },
        ]
    );
    assert_eq!(windows.focused().await.unwrap(), None);
}

#[tokio::test]
async fn replay_rejects_broken_lines() {
    let replay = NiriReplay::default();
    let _app = testkit::app().with(replay.clone()).boot().await.unwrap();
    let err = replay
        .feed(at(0), r#"{"WindowFocusChanged":{"id":"ghostty"}}"#)
        .await
        .unwrap_err();
    assert_eq!(err.kind, ErrorKind::Validation);
}

// ---------- 真实连接，对面是一个假的 niri ----------

struct Rig {
    app: TestApp,
    seen: Seen,
    log: MemoryLog,
    listener: UnixListener,
    _dir: tempfile::TempDir,
}

async fn rig() -> Rig {
    let dir = tempfile::tempdir().unwrap();
    let socket = dir.path().join("niri.sock");
    let listener = UnixListener::bind(&socket).unwrap();
    let (seen, log) = (Seen::default(), MemoryLog::default());
    let app = testkit::app()
        .with(WmNiri)
        .config(
            "wm-niri",
            json!({ "socket": socket.to_string_lossy(), "reconnect_initial_ms": 10, "reconnect_max_ms": 20 }),
        )
        .unwrap()
        .with(FakeClock::at(5_000, 480))
        .with(log.clone())
        .with(seen.clone())
        .background_delivery()
        .boot()
        .await
        .unwrap();
    Rig {
        app,
        seen,
        log,
        listener,
        _dir: dir,
    }
}

/// 接受一个连接，确认对方请求了事件流。
async fn accept(listener: &UnixListener) -> tokio::net::UnixStream {
    let (conn, _) = tokio::time::timeout(Duration::from_secs(5), listener.accept())
        .await
        .unwrap()
        .unwrap();
    let mut reader = BufReader::new(conn);
    let mut request = String::new();
    reader.read_line(&mut request).await.unwrap();
    assert_eq!(request, "\"EventStream\"\n");
    reader.into_inner()
}

async fn wait_for(seen: &Seen, count: usize) {
    for _ in 0..500 {
        if seen.0.lock().unwrap().len() >= count {
            return;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    panic!(
        "等了 2.5 秒只收到 {} 个事件，要 {count} 个",
        seen.0.lock().unwrap().len()
    );
}

#[tokio::test]
async fn reads_the_event_stream_and_reconnects() {
    let rig = rig().await;

    let mut niri = accept(&rig.listener).await;
    niri.write_all(b"{\"Ok\":\"Handled\"}\n").await.unwrap();
    niri.write_all(format!("{SNAPSHOT}\n").as_bytes())
        .await
        .unwrap();
    niri.write_all(b"this line is garbage\n").await.unwrap();
    niri.write_all(b"{\"WindowFocusChanged\":{\"id\":2}}\n")
        .await
        .unwrap();
    wait_for(&rig.seen, 2).await;
    assert_eq!(
        rig.seen.titles(),
        [Some("nvim".to_owned()), Some("Rust".to_owned())]
    );
    assert_eq!(
        rig.seen.0.lock().unwrap()[0].at,
        at(5_000),
        "时刻来自 Clock 契约"
    );
    let clock_reads = rig
        .app
        .requests()
        .iter()
        .filter(|r| r.starts_with("wm-niri -> clock-fake"))
        .count();
    assert_eq!(
        clock_reads, 2,
        "只有焦点真的变了才去问时钟，无关的行和看不懂的行不问"
    );

    // niri 退出：报告没有焦点，然后重连，用新的快照重建
    drop(niri);
    wait_for(&rig.seen, 3).await;
    let mut niri = accept(&rig.listener).await;
    niri.write_all(format!("{SNAPSHOT}\n").as_bytes())
        .await
        .unwrap();
    wait_for(&rig.seen, 4).await;
    assert_eq!(rig.seen.titles()[2..], [None, Some("nvim".to_owned())]);

    let lines = rig.log.lines();
    assert!(
        lines
            .iter()
            .any(|l| l.starts_with("warn wm-niri: niri 的消息看不懂")),
        "{lines:?}"
    );
    assert!(
        lines
            .iter()
            .any(|l| l.contains("niri 关闭了连接") && l.contains("10 毫秒后重连")),
        "{lines:?}"
    );
    rig.app.stop().await;
}

#[tokio::test]
async fn keeps_retrying_while_niri_is_down() {
    let rig = rig().await;
    let socket = rig
        .listener
        .local_addr()
        .unwrap()
        .as_pathname()
        .unwrap()
        .to_owned();
    drop(rig.listener);
    std::fs::remove_file(&socket).unwrap();
    tokio::time::sleep(Duration::from_millis(100)).await;
    let failures = rig
        .log
        .lines()
        .iter()
        .filter(|l| l.contains("连不上"))
        .count();
    assert!(
        failures >= 3,
        "100 毫秒里应该重试了好几次，实际 {failures} 次"
    );
    assert!(
        rig.log.lines().iter().any(|l| l.contains("20 毫秒后重连")),
        "等待时间翻倍到上限"
    );

    // 停止不会被重连循环拖住
    tokio::time::timeout(Duration::from_secs(2), rig.app.stop())
        .await
        .unwrap();
}

/// 接受一个一次性请求的连接，按请求名回复。返回请求名。
async fn answer_one(listener: &UnixListener) -> String {
    let (conn, _) = tokio::time::timeout(Duration::from_secs(5), listener.accept())
        .await
        .unwrap()
        .unwrap();
    let mut reader = BufReader::new(conn);
    let mut request = String::new();
    reader.read_line(&mut request).await.unwrap();
    let reply = match request.as_str() {
        "\"FocusedWindow\"\n" => {
            r#"{"Ok":{"FocusedWindow":{"id":9,"title":"PR #12","app_id":"zen","pid":4321,"workspace_id":3,"is_focused":true}}}"#
        }
        "\"Workspaces\"\n" => r#"{"Ok":{"Workspaces":[{"id":3,"idx":1,"name":"web"}]}}"#,
        other => panic!("没想到的请求 {other:?}"),
    };
    let mut conn = reader.into_inner();
    conn.write_all(format!("{reply}\n").as_bytes())
        .await
        .unwrap();
    request.trim().to_owned()
}

#[tokio::test]
async fn asks_niri_directly_until_the_event_stream_has_the_full_picture() {
    let rig = rig().await;
    let mut niri = accept(&rig.listener).await;
    let windows = rig.app.resolve(&WINDOWS).unwrap();

    // 一次性进程刚启动：事件流还没给完整的窗口列表，直接问
    let asking = tokio::spawn({
        let windows = windows.clone();
        async move { windows.focused().await }
    });
    let mut asked = vec![
        answer_one(&rig.listener).await,
        answer_one(&rig.listener).await,
    ];
    asked.sort();
    assert_eq!(asked, ["\"FocusedWindow\"", "\"Workspaces\""]);
    assert_eq!(
        asking.await.unwrap().unwrap(),
        Some(WindowInfo {
            id: 9,
            app_id: Some("zen".to_owned()),
            title: Some("PR #12".to_owned()),
            workspace: Some("web".to_owned()),
            pid: Some(4321),
        })
    );

    // 有了完整的窗口列表之后用表里的，不再连 niri（要是去连了，没人应答，会超时）
    niri.write_all(format!("{SNAPSHOT}\n").as_bytes())
        .await
        .unwrap();
    wait_for(&rig.seen, 1).await;
    let focused = tokio::time::timeout(Duration::from_secs(1), windows.focused())
        .await
        .expect("不该再去问 niri")
        .unwrap();
    assert_eq!(focused.and_then(|w| w.title), Some("nvim".to_owned()));
    rig.app.stop().await;
}

#[tokio::test]
async fn a_failed_direct_question_is_unavailable() {
    let dir = tempfile::tempdir().unwrap();
    let socket = dir.path().join("nobody-listens.sock");
    let app = testkit::app()
        .with(WmNiri)
        .config("wm-niri", json!({ "socket": socket.to_string_lossy() }))
        .unwrap()
        .with(FakeClock::at(0, 0))
        .with(MemoryLog::default())
        .background_delivery()
        .boot()
        .await
        .unwrap();
    let err = app.resolve(&WINDOWS).unwrap().focused().await.unwrap_err();
    assert_eq!(err.kind, ErrorKind::Unavailable, "{err}");
    app.stop().await;
}

#[tokio::test]
async fn needs_a_socket() {
    // 测试进程可能本来就在 niri 会话里，所以用空字符串明确表示「没有」
    let err = testkit::app()
        .with(WmNiri)
        .config("wm-niri", json!({ "socket": "" }))
        .unwrap()
        .with(FakeClock::at(0, 0))
        .with(MemoryLog::default())
        .boot()
        .await
        .err()
        .unwrap();
    assert_eq!(err.kind, ErrorKind::Config);
    assert!(
        err.message.contains("NIRI_SOCKET") && err.message.contains("enabled = false"),
        "{err}"
    );
}
