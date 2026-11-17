//! niri event-stream 的消息。
//!
//! 每行一个 JSON：`{"事件名": {字段…}}`。niri 有十几种事件，这里只解析判断焦点用得到的 5 种，
//! 其余（包括以后的 niri 新增的）一律跳过。不用 `niri-ipc` crate 的 `Event`：它没有兜底分支，
//! niri 一加新事件，旧版本解析就会失败。

use serde::Deserialize;
use serde_json::Value;

/// 窗口信息。只取用得到的字段，其余字段（布局……）忽略。
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub(crate) struct Window {
    pub id: u64,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub app_id: Option<String>,
    #[serde(default)]
    pub is_focused: bool,
    /// 所在工作区的编号。
    #[serde(default)]
    pub workspace_id: Option<u64>,
    /// 窗口所属进程。
    #[serde(default)]
    pub pid: Option<u32>,
}

/// 工作区。只取编号和名字。
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub(crate) struct Workspace {
    pub id: u64,
    /// 用户给的名字；没命名的是 `None`。
    #[serde(default)]
    pub name: Option<String>,
}

/// 我们关心的 niri 事件。名字和字段与 niri 的 JSON 一致。
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub(crate) enum NiriEvent {
    /// 全部窗口（连接后的第一批消息之一）。没出现在列表里的窗口都已经关闭了。
    WindowsChanged { windows: Vec<Window> },
    /// 新开了一个窗口，或者已有窗口变了（标题、焦点……）。
    WindowOpenedOrChanged { window: Window },
    /// 窗口关闭。
    WindowClosed { id: u64 },
    /// 焦点换到另一个窗口；`None` 表示没有窗口有焦点。
    WindowFocusChanged { id: Option<u64> },
    /// 全部工作区（连接后的第一批消息之一；新建、删除、改名时也会再发）。
    WorkspacesChanged { workspaces: Vec<Workspace> },
}

/// `NiriEvent` 的变体名，和上面的定义保持一致。
const HANDLED: [&str; 5] = [
    "WindowsChanged",
    "WindowOpenedOrChanged",
    "WindowClosed",
    "WindowFocusChanged",
    "WorkspacesChanged",
];

/// 解析一行。不关心的事件返回 `Ok(None)`。
pub(crate) fn parse_line(line: &str) -> Result<Option<NiriEvent>, serde_json::Error> {
    let value: Value = serde_json::from_str(line)?;
    let name = value
        .as_object()
        .and_then(|object| object.keys().next())
        .map(String::as_str);
    match name {
        Some(name) if HANDLED.contains(&name) => serde_json::from_value(value).map(Some),
        _ => Ok(None),
    }
}

/// 一次性请求的回复：成功是 `{"Ok":{"请求名":结果}}`，失败是 `{"Err":"原因"}`。
#[derive(Debug, Deserialize)]
enum Reply<T> {
    Ok(T),
    Err(String),
}

#[derive(Debug, Deserialize)]
struct FocusedWindowReply {
    #[serde(rename = "FocusedWindow")]
    window: Option<Window>,
}

#[derive(Debug, Deserialize)]
struct WorkspacesReply {
    #[serde(rename = "Workspaces")]
    workspaces: Vec<Workspace>,
}

/// 请求 `"FocusedWindow"` 的回复。
pub(crate) fn parse_focused_reply(line: &str) -> Result<Option<Window>, String> {
    match serde_json::from_str::<Reply<FocusedWindowReply>>(line) {
        Ok(Reply::Ok(reply)) => Ok(reply.window),
        Ok(Reply::Err(why)) => Err(why),
        Err(e) => Err(format!("回复看不懂：{e}")),
    }
}

/// 请求 `"Workspaces"` 的回复。
pub(crate) fn parse_workspaces_reply(line: &str) -> Result<Vec<Workspace>, String> {
    match serde_json::from_str::<Reply<WorkspacesReply>>(line) {
        Ok(Reply::Ok(reply)) => Ok(reply.workspaces),
        Ok(Reply::Err(why)) => Err(why),
        Err(e) => Err(format!("回复看不懂：{e}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn replies_to_one_off_requests() {
        let w = parse_focused_reply(
            r#"{"Ok":{"FocusedWindow":{"id":7,"title":"t","app_id":"zen","pid":12,"workspace_id":3,"is_focused":true,"layout":{}}}}"#,
        )
        .unwrap()
        .unwrap();
        assert_eq!((w.id, w.pid, w.workspace_id), (7, Some(12), Some(3)));
        assert_eq!(
            parse_focused_reply(r#"{"Ok":{"FocusedWindow":null}}"#),
            Ok(None)
        );
        assert_eq!(
            parse_workspaces_reply(
                r#"{"Ok":{"Workspaces":[{"id":3,"idx":1,"name":"web","output":"x"}]}}"#
            ),
            Ok(vec![Workspace {
                id: 3,
                name: Some("web".to_owned())
            }])
        );
        assert_eq!(
            parse_focused_reply(r#"{"Err":"nope"}"#),
            Err("nope".to_owned())
        );
        assert!(parse_workspaces_reply("garbage").is_err());
    }

    #[test]
    fn focus_changed() {
        assert_eq!(
            parse_line(r#"{"WindowFocusChanged":{"id":12}}"#).unwrap(),
            Some(NiriEvent::WindowFocusChanged { id: Some(12) })
        );
        assert_eq!(
            parse_line(r#"{"WindowFocusChanged":{"id":null}}"#).unwrap(),
            Some(NiriEvent::WindowFocusChanged { id: None })
        );
    }

    #[test]
    fn window_with_extra_fields() {
        let line = r#"{"WindowOpenedOrChanged":{"window":{"id":3,"title":"◐ nvim","app_id":"com.mitchellh.ghostty","pid":4242,"workspace_id":1,"is_focused":true,"is_floating":false,"is_urgent":false,"layout":{"pos_in_scrolling_layout":[1,1],"tile_size":[960.0,1080.0],"window_size":[958,1078],"tile_pos_in_workspace_view":null,"window_offset_in_tile":[1.0,1.0]},"focus_timestamp":{"secs":123,"nanos":456}}}}"#;
        assert_eq!(
            parse_line(line).unwrap(),
            Some(NiriEvent::WindowOpenedOrChanged {
                window: Window {
                    id: 3,
                    title: Some("◐ nvim".to_owned()),
                    app_id: Some("com.mitchellh.ghostty".to_owned()),
                    is_focused: true,
                    workspace_id: Some(1),
                    pid: Some(4242),
                }
            })
        );
    }

    #[test]
    fn windows_changed_and_closed() {
        let got = parse_line(
            r#"{"WindowsChanged":{"windows":[{"id":1,"title":null,"app_id":"zen","is_focused":false}]}}"#,
        )
        .unwrap();
        assert_eq!(
            got,
            Some(NiriEvent::WindowsChanged {
                windows: vec![Window {
                    id: 1,
                    title: None,
                    app_id: Some("zen".to_owned()),
                    is_focused: false,
                    workspace_id: None,
                    pid: None,
                }]
            })
        );
        assert_eq!(
            parse_line(r#"{"WindowClosed":{"id":1}}"#).unwrap(),
            Some(NiriEvent::WindowClosed { id: 1 })
        );
    }

    #[test]
    fn workspaces_changed() {
        let line = r#"{"WorkspacesChanged":{"workspaces":[{"id":15,"idx":1,"name":"work-weather","output":"eDP-1","is_urgent":false,"is_active":true,"is_focused":true,"active_window_id":3},{"id":16,"idx":2,"name":null,"output":"eDP-1","is_urgent":false,"is_active":false,"is_focused":false,"active_window_id":null}]}}"#;
        assert_eq!(
            parse_line(line).unwrap(),
            Some(NiriEvent::WorkspacesChanged {
                workspaces: vec![
                    Workspace {
                        id: 15,
                        name: Some("work-weather".to_owned())
                    },
                    Workspace { id: 16, name: None },
                ]
            })
        );
    }

    #[test]
    fn other_events_are_skipped() {
        for line in [
            r#"{"ConfigLoaded":{"failed":false}}"#,
            r#"{"SomeEventFromNiri27":{"x":1}}"#,
            r#"{"Ok":"Handled"}"#,
        ] {
            assert_eq!(parse_line(line).unwrap(), None, "{line}");
        }
    }

    #[test]
    fn broken_lines_are_errors() {
        assert!(parse_line("not json").is_err());
        assert!(parse_line(r#"{"WindowFocusChanged":{"id":"abc"}}"#).is_err());
        assert!(parse_line(r#"{"WindowClosed":{}}"#).is_err());
    }
}
