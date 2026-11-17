//! 契约：按 pid 查进程。
//!
//! 只描述进程本身（父子关系、会话、终端、命令行、工作目录），不认识终端模拟器、shell 这些词：
//! 「哪个是前台命令」由使用方根据这些数据判断。

use kernel_api::{AppError, contract};
use serde::{Deserialize, Serialize};

/// 一个进程。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProcessInfo {
    pub pid: u32,
    pub ppid: u32,
    /// 进程组。
    pub pgrp: u32,
    /// 会话；等于 `pid` 时它是会话首进程（比如终端里的 shell）。
    pub session: u32,
    /// 控制终端的设备号；没有控制终端是 `None`。
    #[serde(default)]
    pub tty: Option<u32>,
    /// 控制终端上的前台进程组；没有控制终端是 `None`。
    #[serde(default)]
    pub foreground_pgrp: Option<u32>,
    /// 启动时刻，单位不定，只用来比先后。
    pub started: u64,
    /// 进程名（内核里的短名字）。
    pub name: String,
    /// 命令行参数；内核线程、僵尸进程是空的。
    #[serde(default)]
    pub cmdline: Vec<String>,
    /// 工作目录；读不到（权限、已退出）是 `None`。
    #[serde(default)]
    pub cwd: Option<String>,
}

contract! {
    /// 进程信息。
    pub trait Processes as PROCESSES = service("process", 1) {
        /// 一个进程。不存在是 `not_found`。
        async fn info(&self, pid: u32) -> Result<ProcessInfo, AppError>;

        /// 直接子进程，按 pid 排序。进程不存在是 `not_found`。
        async fn children(&self, pid: u32) -> Result<Vec<ProcessInfo>, AppError>;
    }
}

#[cfg(feature = "testing")]
pub mod testing {
    //! 契约测试：拿正在跑测试的进程自己来查。

    use std::sync::Arc;

    use kernel_api::{AppError, ErrorKind};

    use super::Processes;

    fn check(ok: bool, what: &str) -> Result<(), AppError> {
        if ok {
            Ok(())
        } else {
            Err(AppError::validation(format!("契约不满足：{what}")))
        }
    }

    /// 任何实现都要通过。
    pub async fn process_contract(processes: Arc<dyn Processes>) -> Result<(), AppError> {
        let me = std::process::id();
        let info = processes.info(me).await?;
        check(info.pid == me, "pid 原样")?;
        check(!info.cmdline.is_empty(), "查得到命令行")?;
        let cwd = std::env::current_dir()
            .map_err(|e| AppError::unavailable(e.to_string()))?
            .to_string_lossy()
            .into_owned();
        check(info.cwd.as_deref() == Some(cwd.as_str()), "查得到工作目录")?;
        let siblings = processes.children(info.ppid).await?;
        check(
            siblings.iter().any(|p| p.pid == me),
            "父进程的子进程里有自己",
        )?;
        check(
            siblings.windows(2).all(|w| w[0].pid < w[1].pid),
            "子进程按 pid 排序",
        )?;
        for pid in [u32::MAX - 1, 0] {
            let err = processes.info(pid).await;
            check(
                matches!(err, Err(ref e) if e.kind == ErrorKind::NotFound),
                "不存在的 pid 是 not_found",
            )?;
            let err = processes.children(pid).await;
            check(
                matches!(err, Err(ref e) if e.kind == ErrorKind::NotFound),
                "不存在的 pid 的子进程是 not_found",
            )?;
        }
        Ok(())
    }
}
