//! 系统服务 `prompt-vicinae`：用 `vicinae dmenu` 弹输入框，提供 `Prompt`。
//!
//! vicinae 相关的东西全在这个包里：命令、参数、退出码的含义。
//!
//! 实测的行为（2026-11-17）：
//! - 输入的文字不匹配任何列表项时回车，stdout 是输入的原文，退出码 0；
//! - 列表非空时空回车会输出第一项，所以 stdin 永远给空的，列表里什么都没有；
//! - Esc：没有输出，退出码 1。

use std::process::Stdio;
use std::sync::Arc;

use kernel_api::{
    AppError, Component, ConfigSection, Host, Kind, Manifest, async_trait, register_component,
};
use prompt_contract::{Answer, PROMPT, Prompt, PromptRequest};
use serde::{Deserialize, Serialize};
use tokio::process::Command;

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields, default)]
struct Config {
    /// vicinae 的命令，不在 PATH 里时写完整路径。
    command: String,
    /// 窗口宽度（像素）。不写用 vicinae 的默认值。
    width: Option<u32>,
    /// 窗口高度（像素）。
    height: Option<u32>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            command: "vicinae".to_owned(),
            width: None,
            height: None,
        }
    }
}

/// vicinae 的退出码 1 表示用户取消。
const EXIT_CANCELLED: i32 = 1;

struct Vicinae(Config);

impl Vicinae {
    fn args(&self, request: &PromptRequest) -> Vec<String> {
        let mut args = vec![
            "dmenu".to_owned(),
            "--placeholder".to_owned(),
            request.placeholder.clone(),
            "--no-section".to_owned(),
            "--no-quick-look".to_owned(),
        ];
        if !request.initial.is_empty() {
            args.extend(["--query".to_owned(), request.initial.clone()]);
        }
        if let Some(w) = self.0.width {
            args.extend(["--width".to_owned(), w.to_string()]);
        }
        if let Some(h) = self.0.height {
            args.extend(["--height".to_owned(), h.to_string()]);
        }
        args
    }
}

#[async_trait]
impl Prompt for Vicinae {
    async fn ask(&self, request: PromptRequest) -> Result<Answer, AppError> {
        let command = &self.0.command;
        let output = Command::new(command)
            .args(self.args(&request))
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            // 调用方不等了（比如进程要退出），输入框也跟着关掉
            .kill_on_drop(true)
            .output()
            .await
            .map_err(|e| AppError::unavailable(format!("启动 `{command}` 失败：{e}")))?;
        match output.status.code() {
            Some(0) => Ok(Answer::from_raw(&String::from_utf8_lossy(&output.stdout))),
            Some(EXIT_CANCELLED) => Ok(Answer::Cancelled),
            code => Err(AppError::unavailable(format!(
                "`{command} dmenu` 异常退出（{}）：{}",
                code.map_or_else(|| "被信号终止".to_owned(), |c| format!("退出码 {c}")),
                String::from_utf8_lossy(&output.stderr).trim()
            ))),
        }
    }
}

pub struct PromptVicinae;

impl Component for PromptVicinae {
    fn config_defaults(&self) -> Option<ConfigSection> {
        ConfigSection::defaults::<Config>()
    }

    fn manifest(&self) -> Manifest {
        Manifest {
            id: "prompt-vicinae",
            kind: Kind::Service,
            summary: "用 vicinae dmenu 弹输入框",
            provides: vec![PROMPT.id()],
            requires: vec![],
            optional_requires: vec![],
        }
    }

    fn start(&self, config: &ConfigSection, host: &mut Host<'_>) -> Result<(), AppError> {
        let config: Config = config.parse()?;
        if config.command.trim().is_empty() {
            return Err(AppError::config("prompt-vicinae 的 command 不能是空的"));
        }
        host.provide(&PROMPT, Arc::new(Vicinae(config)))
    }
}

register_component!(PromptVicinae);

#[cfg(test)]
mod tests;
