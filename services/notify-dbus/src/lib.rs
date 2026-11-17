//! 系统服务 `notify-dbus`：通过 D-Bus 的 `org.freedesktop.Notifications` 发通知。
//!
//! D-Bus 只出现在这个包里。不调 `notify-send`：少一个外部程序，错误也拿得到。

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use kernel_api::{
    AppError, Component, ConfigSection, Host, Kind, Manifest, async_trait, register_component,
};
use notify_contract::{NOTIFY, Notification, Notify, Urgency};
use serde::{Deserialize, Serialize};
use zbus::zvariant::Value;

const DEST: &str = "org.freedesktop.Notifications";
const PATH: &str = "/org/freedesktop/Notifications";

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields, default)]
struct Config {
    /// 通知上显示的应用名。启动器会给默认值。
    app_name: String,
    /// 总线地址；不写用会话总线（`DBUS_SESSION_BUS_ADDRESS`）。
    bus: Option<String>,
    /// 通知显示多久（毫秒）；-1 由通知服务决定，0 一直显示。
    expire_ms: i32,
    /// 连总线、等回复最多多久。
    timeout_ms: u64,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            app_name: String::new(),
            bus: None,
            expire_ms: -1,
            timeout_ms: 3_000,
        }
    }
}

/// 规范里的紧急程度：0 低、1 普通、2 紧急。
const fn urgency(u: Urgency) -> u8 {
    match u {
        Urgency::Low => 0,
        Urgency::Normal => 1,
        Urgency::Critical => 2,
    }
}

struct Dbus(Config);

fn unavailable(e: &zbus::Error) -> AppError {
    AppError::unavailable(format!("发通知失败：{e}"))
}

impl Dbus {
    async fn connect(&self) -> Result<zbus::Connection, AppError> {
        match &self.0.bus {
            Some(address) => zbus::connection::Builder::address(address.as_str())
                .map_err(|e| unavailable(&e))?
                .build()
                .await
                .map_err(|e| unavailable(&e)),
            None => zbus::Connection::session()
                .await
                .map_err(|e| unavailable(&e)),
        }
    }

    async fn notify(&self, n: Notification) -> Result<(), AppError> {
        let conn = self.connect().await?;
        let hints: HashMap<&str, Value<'_>> =
            HashMap::from([("urgency", Value::U8(urgency(n.urgency)))]);
        let actions: Vec<&str> = Vec::new();
        conn.call_method(
            Some(DEST),
            PATH,
            Some(DEST),
            "Notify",
            &(
                self.0.app_name.as_str(),
                0u32,
                "",
                n.summary.as_str(),
                n.body.as_str(),
                actions,
                hints,
                self.0.expire_ms,
            ),
        )
        .await
        .map_err(|e| unavailable(&e))?;
        Ok(())
    }
}

#[async_trait]
impl Notify for Dbus {
    async fn send(&self, n: Notification) -> Result<(), AppError> {
        if n.summary.trim().is_empty() {
            return Err(AppError::validation("通知的标题不能是空的"));
        }
        let limit = Duration::from_millis(self.0.timeout_ms);
        tokio::time::timeout(limit, self.notify(n))
            .await
            .map_err(|_| {
                AppError::unavailable(format!("{} 毫秒内通知服务没有回应", self.0.timeout_ms))
            })?
    }
}

pub struct NotifyDbus;

impl Component for NotifyDbus {
    fn config_defaults(&self) -> Option<ConfigSection> {
        ConfigSection::defaults::<Config>()
    }

    fn manifest(&self) -> Manifest {
        Manifest {
            id: "notify-dbus",
            kind: Kind::Service,
            summary: "通过 D-Bus 发桌面通知",
            provides: vec![NOTIFY.id()],
            requires: vec![],
            optional_requires: vec![],
        }
    }

    fn start(&self, config: &ConfigSection, host: &mut Host<'_>) -> Result<(), AppError> {
        let config: Config = config.parse()?;
        host.provide(&NOTIFY, Arc::new(Dbus(config)))
    }
}

register_component!(NotifyDbus);

#[cfg(test)]
mod tests;
