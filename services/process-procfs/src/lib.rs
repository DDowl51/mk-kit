//! 系统服务 `process-procfs`：读 `/proc`，提供 `Processes`。

use std::path::{Path, PathBuf};
use std::sync::Arc;

use kernel_api::{
    AppError, Component, ConfigSection, Host, Kind, Manifest, async_trait, register_component,
};
use process_contract::{PROCESSES, ProcessInfo, Processes};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields, default)]
struct Config {
    /// proc 文件系统挂在哪。测试时换成假的目录。
    root: String,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            root: "/proc".to_owned(),
        }
    }
}

/// `/proc/<pid>/stat` 里括号后面的部分：`state ppid pgrp session tty_nr tpgid … starttime …`。
/// 进程名在括号里，可能带空格和括号，所以从最后一个 `)` 往后切。
fn parse_stat(pid: u32, stat: &str) -> Option<ProcessInfo> {
    let open = stat.find('(')?;
    let close = stat.rfind(')')?;
    let name = stat.get(open + 1..close)?.to_owned();
    let rest: Vec<&str> = stat.get(close + 1..)?.split_whitespace().collect();
    let num = |i: usize| rest.get(i).and_then(|v| v.parse::<i64>().ok());
    let positive = |v: i64| u32::try_from(v).ok().filter(|v| *v > 0);
    Some(ProcessInfo {
        pid,
        ppid: u32::try_from(num(1)?).ok()?,
        pgrp: u32::try_from(num(2)?).ok()?,
        session: u32::try_from(num(3)?).ok()?,
        tty: num(4).and_then(positive),
        foreground_pgrp: num(5).and_then(positive),
        started: u64::try_from(num(19)?).ok()?,
        name,
        cmdline: Vec::new(),
        cwd: None,
    })
}

/// `cmdline` 是 NUL 分隔的。
fn parse_cmdline(raw: &[u8]) -> Vec<String> {
    raw.split(|b| *b == 0)
        .filter(|part| !part.is_empty())
        .map(|part| String::from_utf8_lossy(part).into_owned())
        .collect()
}

struct Procfs {
    root: PathBuf,
}

impl Procfs {
    fn dir(&self, pid: u32) -> PathBuf {
        self.root.join(pid.to_string())
    }

    /// 读一个进程。不存在（或者刚好退出）是 `None`。
    fn read(&self, pid: u32) -> Option<ProcessInfo> {
        let dir = self.dir(pid);
        let stat = std::fs::read_to_string(dir.join("stat")).ok()?;
        let mut info = parse_stat(pid, &stat)?;
        info.cmdline = std::fs::read(dir.join("cmdline"))
            .map(|raw| parse_cmdline(&raw))
            .unwrap_or_default();
        info.cwd = std::fs::read_link(dir.join("cwd"))
            .ok()
            .map(|p| p.to_string_lossy().into_owned());
        Some(info)
    }

    fn info(&self, pid: u32) -> Result<ProcessInfo, AppError> {
        if pid == 0 {
            return Err(AppError::not_found("没有 pid 0 这个进程"));
        }
        self.read(pid)
            .ok_or_else(|| AppError::not_found(format!("没有 pid {pid} 这个进程")))
    }

    /// 扫一遍所有进程找父进程是 `pid` 的。`/proc/<pid>/task/*/children` 要内核选项，扫描更可靠。
    fn children(&self, pid: u32) -> Result<Vec<ProcessInfo>, AppError> {
        self.info(pid)?;
        let entries = std::fs::read_dir(&self.root)
            .map_err(|e| AppError::unavailable(format!("读 {} 失败：{e}", self.root.display())))?;
        let mut out: Vec<ProcessInfo> = entries
            .flatten()
            .filter_map(|e| e.file_name().to_str()?.parse::<u32>().ok())
            .filter_map(|child| {
                // 先只读 stat 判断父子，是的话再读全
                let stat = std::fs::read_to_string(self.dir(child).join("stat")).ok()?;
                (parse_stat(child, &stat)?.ppid == pid).then(|| self.read(child))?
            })
            .collect();
        out.sort_by_key(|p| p.pid);
        Ok(out)
    }
}

/// 在阻塞线程里跑，别卡住异步运行时。
async fn blocking<T, F>(procfs: Arc<Procfs>, f: F) -> Result<T, AppError>
where
    T: Send + 'static,
    F: FnOnce(&Procfs) -> Result<T, AppError> + Send + 'static,
{
    tokio::task::spawn_blocking(move || f(&procfs))
        .await
        .map_err(|e| AppError::unavailable(format!("读进程信息的线程崩了：{e}")))?
}

struct Service(Arc<Procfs>);

#[async_trait]
impl Processes for Service {
    async fn info(&self, pid: u32) -> Result<ProcessInfo, AppError> {
        blocking(self.0.clone(), move |p| p.info(pid)).await
    }

    async fn children(&self, pid: u32) -> Result<Vec<ProcessInfo>, AppError> {
        blocking(self.0.clone(), move |p| p.children(pid)).await
    }
}

pub struct ProcessProcfs;

impl Component for ProcessProcfs {
    fn config_defaults(&self) -> Option<ConfigSection> {
        ConfigSection::defaults::<Config>()
    }

    fn manifest(&self) -> Manifest {
        Manifest {
            id: "process-procfs",
            kind: Kind::Service,
            summary: "读 /proc：按 pid 查进程",
            provides: vec![PROCESSES.id()],
            requires: vec![],
            optional_requires: vec![],
        }
    }

    fn start(&self, config: &ConfigSection, host: &mut Host<'_>) -> Result<(), AppError> {
        let config: Config = config.parse()?;
        let root = PathBuf::from(&config.root);
        if !Path::new(&root).is_absolute() {
            return Err(AppError::config(format!(
                "process-procfs 的 root `{}` 要是绝对路径",
                config.root
            )));
        }
        host.provide(&PROCESSES, Arc::new(Service(Arc::new(Procfs { root }))))
    }
}

register_component!(ProcessProcfs);

#[cfg(test)]
mod tests {
    use process_contract::testing::process_contract;

    use super::*;

    #[test]
    fn stat_lines() {
        // 进程名里有空格和括号；tty_nr 0 和 tpgid -1 表示没有终端
        let got = parse_stat(
            42,
            "42 (my (odd) name) S 1 42 42 0 -1 4194560 100 0 0 0 1 2 0 0 20 0 1 0 12345 1000 10",
        )
        .unwrap();
        assert_eq!(got.name, "my (odd) name");
        assert_eq!((got.ppid, got.pgrp, got.session), (1, 42, 42));
        assert_eq!((got.tty, got.foreground_pgrp), (None, None));
        assert_eq!(got.started, 12345);
        let got = parse_stat(
            7,
            "7 (fish) S 3 7 7 34817 9 4194304 0 0 0 0 0 0 0 0 20 0 1 0 999 0 0",
        )
        .unwrap();
        assert_eq!((got.tty, got.foreground_pgrp), (Some(34817), Some(9)));
        assert!(parse_stat(1, "garbage").is_none());
        assert!(parse_stat(1, "1 (x) S").is_none());
    }

    #[test]
    fn cmdlines() {
        assert_eq!(
            parse_cmdline(b"nvim\0src/main.rs\0"),
            ["nvim", "src/main.rs"]
        );
        assert!(parse_cmdline(b"").is_empty());
    }

    #[tokio::test]
    async fn passes_the_process_contract() {
        let app = testkit::app().with(ProcessProcfs).boot().await.unwrap();
        process_contract(app.resolve(&PROCESSES).unwrap())
            .await
            .unwrap();
        app.stop().await;
    }

    #[tokio::test]
    async fn root_must_be_absolute() {
        let err = testkit::app()
            .with(ProcessProcfs)
            .config("process-procfs", serde_json::json!({ "root": "proc" }))
            .unwrap()
            .boot()
            .await
            .err()
            .unwrap();
        assert_eq!(err.kind, kernel_api::ErrorKind::Config);
    }
}
