//! 系统服务 `screenshot-niri`：用 `niri msg action screenshot-window` 截窗口，提供 `Screenshots`。
//!
//! niri 的怪癖都关在这里：
//! - 它截图时**总会同时写剪贴板**，没有关掉的选项。契约要求没有可见副作用，所以截图前用
//!   `wl-paste` 存下剪贴板的首选类型，截完用 `wl-copy` 放回去；原来是空的就清掉。
//!   已知副作用：剪贴板管理器的历史里会多一张截图；只恢复首选的那一种类型。
//! - 命令返回时文件可能还没写完，所以等到文件出现、大小稳定、开头是 PNG 为止。

use std::path::Path;
use std::process::{Output, Stdio};
use std::sync::Arc;
use std::time::Duration;

use kernel_api::{
    AppError, Component, ConfigSection, Host, Kind, Manifest, async_trait, register_component,
};
use screenshot_contract::{SCREENSHOTS, Screenshots};
use serde::{Deserialize, Serialize};
use tokio::io::AsyncWriteExt;
use tokio::process::Command;
use tokio::time::{Instant, sleep, timeout};

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields, default)]
struct Config {
    niri: String,
    wl_paste: String,
    wl_copy: String,
    /// 截完把剪贴板恢复成截图之前的样子。
    restore_clipboard: bool,
    /// 等截图文件写完最多多久。
    timeout_ms: u64,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            niri: "niri".to_owned(),
            wl_paste: "wl-paste".to_owned(),
            wl_copy: "wl-copy".to_owned(),
            restore_clipboard: true,
            timeout_ms: 3_000,
        }
    }
}

const PNG_SIGNATURE: [u8; 8] = [0x89, b'P', b'N', b'G', b'\r', b'\n', 0x1a, b'\n'];
const POLL: Duration = Duration::from_millis(20);

async fn run(command: &str, args: &[&str]) -> Result<Output, AppError> {
    Command::new(command)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .output()
        .await
        .map_err(|e| AppError::unavailable(format!("运行 `{command}` 失败：{e}")))
}

fn failed(command: &str, out: &Output) -> AppError {
    AppError::unavailable(format!(
        "`{command}` 失败（{}）：{}",
        out.status,
        String::from_utf8_lossy(&out.stderr).trim()
    ))
}

/// 截图之前的剪贴板：（类型，内容）；空的是 `None`。
type Clipboard = Option<(String, Vec<u8>)>;

struct Niri(Config);

impl Niri {
    async fn window_exists(&self, id: u64) -> Result<bool, AppError> {
        let out = run(&self.0.niri, &["msg", "-j", "windows"]).await?;
        if !out.status.success() {
            return Err(failed(&self.0.niri, &out));
        }
        let windows: Vec<serde_json::Value> = serde_json::from_slice(&out.stdout)
            .map_err(|e| AppError::unavailable(format!("niri 的窗口列表看不懂：{e}")))?;
        Ok(windows.iter().any(|w| w["id"].as_u64() == Some(id)))
    }

    /// 读不到（没有 wl-paste、剪贴板是空的）都当空的。
    async fn save_clipboard(&self) -> Clipboard {
        let types = run(&self.0.wl_paste, &["--list-types"]).await.ok()?;
        let first = String::from_utf8_lossy(&types.stdout)
            .lines()
            .map(str::trim)
            .find(|l| !l.is_empty())?
            .to_owned();
        if !types.status.success() {
            return None;
        }
        let content = run(&self.0.wl_paste, &["--no-newline", "--type", &first])
            .await
            .ok()
            .filter(|o| o.status.success())?;
        Some((first, content.stdout))
    }

    /// 放回去。失败不影响截图本身（图已经写好了），所以只返回原因给调用方决定。
    async fn restore_clipboard(&self, saved: Clipboard) -> Result<(), AppError> {
        let fail = |e: std::io::Error| AppError::unavailable(format!("恢复剪贴板失败：{e}"));
        let args: Vec<&str> = match &saved {
            Some((kind, _)) => vec!["--type", kind.as_str()],
            None => vec!["--clear"],
        };
        let mut child = Command::new(&self.0.wl_copy)
            .args(&args)
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .map_err(fail)?;
        if let Some(mut stdin) = child.stdin.take() {
            if let Some((_, content)) = &saved {
                stdin.write_all(content).await.map_err(fail)?;
            }
            drop(stdin);
        }
        // wl-copy 自己会转到后台去守着剪贴板，前台进程很快退出
        match timeout(Duration::from_secs(2), child.wait()).await {
            Ok(status) => {
                status.map_err(fail)?;
                Ok(())
            }
            Err(_) => Err(AppError::unavailable("恢复剪贴板超时")),
        }
    }

    /// 等文件写完：出现、非空、大小两次一样、开头是 PNG。
    async fn wait_for(&self, path: &Path) -> Result<(), AppError> {
        let deadline = Instant::now() + Duration::from_millis(self.0.timeout_ms);
        let mut last = None;
        while Instant::now() < deadline {
            if let Ok(meta) = tokio::fs::metadata(path).await
                && meta.len() > 0
            {
                if last == Some(meta.len()) {
                    let head = tokio::fs::read(path).await.unwrap_or_default();
                    if head.starts_with(&PNG_SIGNATURE) {
                        return Ok(());
                    }
                }
                last = Some(meta.len());
            }
            sleep(POLL).await;
        }
        Err(AppError::unavailable(format!(
            "{} 毫秒内没等到截图文件 {}",
            self.0.timeout_ms,
            path.display()
        )))
    }
}

#[async_trait]
impl Screenshots for Niri {
    async fn window(&self, window_id: u64, path: String) -> Result<(), AppError> {
        let file = Path::new(&path);
        if !file.is_absolute() {
            return Err(AppError::validation(format!(
                "截图路径 `{path}` 要是绝对路径"
            )));
        }
        if !self.window_exists(window_id).await? {
            return Err(AppError::not_found(format!("没有窗口 {window_id}")));
        }
        let io = |e: std::io::Error| AppError::unavailable(format!("准备 {path} 失败：{e}"));
        if let Some(dir) = file.parent() {
            tokio::fs::create_dir_all(dir).await.map_err(io)?;
        }
        // 旧文件会让「等文件出现」立刻成立
        match tokio::fs::remove_file(file).await {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => return Err(io(e)),
            _ => {}
        }
        let saved = if self.0.restore_clipboard {
            Some(self.save_clipboard().await)
        } else {
            None
        };
        let id = window_id.to_string();
        let out = run(
            &self.0.niri,
            &[
                "msg",
                "action",
                "screenshot-window",
                "--id",
                &id,
                "--path",
                &path,
            ],
        )
        .await?;
        if !out.status.success() {
            return Err(failed(&self.0.niri, &out));
        }
        let written = self.wait_for(file).await;
        if let Some(saved) = saved {
            // 截图失败也要恢复：niri 可能已经改了剪贴板
            self.restore_clipboard(saved).await?;
        }
        written
    }

    async fn remove(&self, path: String) -> Result<bool, AppError> {
        match tokio::fs::remove_file(&path).await {
            Ok(()) => Ok(true),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
            Err(e) => Err(AppError::unavailable(format!("删 {path} 失败：{e}"))),
        }
    }
}

pub struct ScreenshotNiri;

impl Component for ScreenshotNiri {
    fn config_defaults(&self) -> Option<ConfigSection> {
        ConfigSection::defaults::<Config>()
    }

    fn manifest(&self) -> Manifest {
        Manifest {
            id: "screenshot-niri",
            kind: Kind::Service,
            summary: "用 niri 截窗口（并恢复被 niri 改掉的剪贴板）",
            provides: vec![SCREENSHOTS.id()],
            requires: vec![],
            optional_requires: vec![],
        }
    }

    fn start(&self, config: &ConfigSection, host: &mut Host<'_>) -> Result<(), AppError> {
        let config: Config = config.parse()?;
        if [&config.niri, &config.wl_paste, &config.wl_copy]
            .iter()
            .any(|c| c.trim().is_empty())
        {
            return Err(AppError::config("screenshot-niri 的命令不能是空的"));
        }
        host.provide(&SCREENSHOTS, Arc::new(Niri(config)))
    }
}

register_component!(ScreenshotNiri);

#[cfg(test)]
mod tests;
