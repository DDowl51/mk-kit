//! 契约：发一条桌面通知。
//!
//! 描述的是能力，不是某个通知协议：这里没有 D-Bus 之类的词。

use kernel_api::{AppError, contract};
use serde::{Deserialize, Serialize};

/// 紧急程度。具体怎么显示由通知服务决定。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Urgency {
    Low,
    #[default]
    Normal,
    Critical,
}

/// 一条通知。
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Notification {
    /// 标题，一行。
    pub summary: String,
    /// 正文，可以多行。
    #[serde(default)]
    pub body: String,
    #[serde(default)]
    pub urgency: Urgency,
}

contract! {
    /// 桌面通知。
    pub trait Notify as NOTIFY = service("notify", 1) {
        /// 发出去就返回，不等用户看。没有通知服务（不在桌面会话里）是 `unavailable`；
        /// 标题是空的是 `validation`。
        async fn send(&self, notification: Notification) -> Result<(), AppError>;
    }
}

#[cfg(feature = "testing")]
pub mod testing {
    //! 记录发出的通知的假实现。

    use std::sync::{Arc, Mutex, PoisonError};

    use kernel_api::{AppError, Component, ConfigSection, Host, Kind, Manifest, async_trait};

    use super::{NOTIFY, Notification, Notify};

    /// 假通知：记下发出的通知。`failing` 时每次都返回 `unavailable`。克隆出来的是同一个。
    #[derive(Clone, Default)]
    pub struct RecordingNotify {
        sent: Arc<Mutex<Vec<Notification>>>,
        failing: bool,
    }

    impl RecordingNotify {
        /// 像没有通知服务一样，每次都失败。
        pub fn failing() -> Self {
            Self {
                failing: true,
                ..Self::default()
            }
        }

        pub fn sent(&self) -> Vec<Notification> {
            self.sent
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .clone()
        }
    }

    #[async_trait]
    impl Notify for RecordingNotify {
        async fn send(&self, notification: Notification) -> Result<(), AppError> {
            if self.failing {
                return Err(AppError::unavailable("没有通知服务"));
            }
            if notification.summary.trim().is_empty() {
                return Err(AppError::validation("通知的标题不能是空的"));
            }
            self.sent
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .push(notification);
            Ok(())
        }
    }

    /// 组件 `notify-fake`。
    impl Component for RecordingNotify {
        fn manifest(&self) -> Manifest {
            Manifest {
                id: "notify-fake",
                kind: Kind::Service,
                summary: "测试用：只记录的假通知",
                provides: vec![NOTIFY.id()],
                requires: vec![],
                optional_requires: vec![],
            }
        }

        fn start(&self, _config: &ConfigSection, host: &mut Host<'_>) -> Result<(), AppError> {
            host.provide(&NOTIFY, Arc::new(self.clone()))
        }
    }
}
