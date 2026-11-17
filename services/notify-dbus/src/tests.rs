//! 起一个私有的 dbus-daemon，在上面放一个假的通知服务，看发过去的是什么。

use std::io::{BufRead, BufReader};
use std::process::{Child, Command, Stdio};
use std::sync::Mutex;

use kernel_api::ErrorKind;
use zbus::zvariant::OwnedValue;

use super::*;

/// 私有总线。drop 时关掉。
struct Bus {
    child: Child,
    address: String,
}

impl Drop for Bus {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn bus() -> Bus {
    let mut child = Command::new("dbus-daemon")
        .args(["--session", "--nofork", "--print-address=1"])
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("要装 dbus-daemon");
    let mut address = String::new();
    BufReader::new(child.stdout.take().unwrap())
        .read_line(&mut address)
        .unwrap();
    Bus {
        child,
        address: address.trim().to_owned(),
    }
}

/// 收到的通知：（应用名，标题，正文，紧急程度，显示多久）。
type Got = Vec<(String, String, String, Option<u8>, i32)>;

#[derive(Clone, Default)]
struct Server(Arc<Mutex<Got>>);

#[zbus::interface(name = "org.freedesktop.Notifications")]
impl Server {
    #[allow(clippy::too_many_arguments)]
    async fn notify(
        &self,
        app_name: String,
        _replaces_id: u32,
        _app_icon: String,
        summary: String,
        body: String,
        _actions: Vec<String>,
        hints: HashMap<String, OwnedValue>,
        expire_timeout: i32,
    ) -> u32 {
        let urgency = hints.get("urgency").and_then(|v| u8::try_from(v).ok());
        let mut got = self.0.lock().unwrap();
        got.push((app_name, summary, body, urgency, expire_timeout));
        u32::try_from(got.len()).unwrap()
    }
}

async fn serve(bus: &Bus) -> (zbus::Connection, Server) {
    let server = Server::default();
    let conn = zbus::connection::Builder::address(bus.address.as_str())
        .unwrap()
        .name(DEST)
        .unwrap()
        .serve_at(PATH, server.clone())
        .unwrap()
        .build()
        .await
        .unwrap();
    (conn, server)
}

fn dbus(bus: &Bus) -> Dbus {
    Dbus(Config {
        app_name: "stash".to_owned(),
        bus: Some(bus.address.clone()),
        ..Config::default()
    })
}

#[tokio::test]
async fn sends_to_the_notification_service() {
    let bus = bus();
    let (_conn, server) = serve(&bus).await;
    let n = dbus(&bus);
    n.send(Notification {
        summary: "收件箱有 7 条".to_owned(),
        body: "整理一下：stash-tui".to_owned(),
        urgency: Urgency::Normal,
    })
    .await
    .unwrap();
    n.send(Notification {
        summary: "交周报 过期了".to_owned(),
        body: String::new(),
        urgency: Urgency::Critical,
    })
    .await
    .unwrap();
    let got = server.0.lock().unwrap().clone();
    assert_eq!(
        got,
        [
            (
                "stash".to_owned(),
                "收件箱有 7 条".to_owned(),
                "整理一下：stash-tui".to_owned(),
                Some(1),
                -1
            ),
            (
                "stash".to_owned(),
                "交周报 过期了".to_owned(),
                String::new(),
                Some(2),
                -1
            ),
        ]
    );
}

#[tokio::test]
async fn no_notification_service_is_unavailable() {
    let bus = bus();
    let err = dbus(&bus)
        .send(Notification {
            summary: "x".to_owned(),
            ..Notification::default()
        })
        .await
        .unwrap_err();
    assert_eq!(err.kind, ErrorKind::Unavailable, "{err}");

    let err = Dbus(Config {
        bus: Some("unix:path=/no/such/bus".to_owned()),
        ..Config::default()
    })
    .send(Notification {
        summary: "x".to_owned(),
        ..Notification::default()
    })
    .await
    .unwrap_err();
    assert_eq!(err.kind, ErrorKind::Unavailable, "{err}");
}

#[tokio::test]
async fn an_empty_summary_is_rejected_before_sending() {
    let err = Dbus(Config::default())
        .send(Notification::default())
        .await
        .unwrap_err();
    assert_eq!(err.kind, ErrorKind::Validation);
}

#[tokio::test]
async fn config_is_checked() {
    let err = testkit::app()
        .with(NotifyDbus)
        .config("notify-dbus", serde_json::json!({ "colour": "red" }))
        .unwrap()
        .boot()
        .await
        .err()
        .unwrap();
    assert_eq!(err.kind, ErrorKind::Config);
}
