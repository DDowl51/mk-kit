//! 系统服务 `exec-tokio`：用 tokio 起子进程，提供 `Exec`。

use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;

use exec_contract::{CommandOutput, CommandRequest, EXEC, Exec};
use kernel_api::{
    AppError, Component, ConfigSection, Host, Kind, Manifest, NoConfig, async_trait,
    register_component,
};
use tokio::io::AsyncWriteExt;
use tokio::process::Command;

struct Runner;

#[async_trait]
impl Exec for Runner {
    async fn run(&self, request: CommandRequest) -> Result<CommandOutput, AppError> {
        let Some((program, args)) = request.argv.split_first() else {
            return Err(AppError::validation("要运行的命令是空的"));
        };
        let mut child = Command::new(program)
            .args(args)
            .envs(request.env.iter().map(|(k, v)| (k, v)))
            .stdin(if request.stdin.is_some() {
                Stdio::piped()
            } else {
                Stdio::null()
            })
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            // 超时、调用方放弃时都杀掉
            .kill_on_drop(true)
            .spawn()
            .map_err(|e| AppError::unavailable(format!("启动 `{program}` 失败：{e}")))?;
        if let (Some(input), Some(mut stdin)) = (request.stdin, child.stdin.take()) {
            // 另起一个任务写：输入大、对方又先往 stdout 写满管道时，同步地先写完会互相等死。
            // 对方不读 stdin 就退出时写会失败，那不算错
            tokio::spawn(async move {
                let _ = stdin.write_all(input.as_bytes()).await;
            });
        }
        let limit = Duration::from_millis(request.timeout_ms);
        let out = tokio::time::timeout(limit, child.wait_with_output())
            .await
            .map_err(|_| {
                AppError::temp_fail(format!(
                    "`{program}` {} 毫秒没跑完，杀掉了",
                    request.timeout_ms
                ))
            })?
            .map_err(|e| AppError::unavailable(format!("等 `{program}` 失败：{e}")))?;
        Ok(CommandOutput {
            code: out.status.code(),
            stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
        })
    }
}

pub struct ExecTokio;

impl Component for ExecTokio {
    fn config_defaults(&self) -> Option<ConfigSection> {
        ConfigSection::defaults::<NoConfig>()
    }

    fn manifest(&self) -> Manifest {
        Manifest {
            id: "exec-tokio",
            kind: Kind::Service,
            summary: "运行外部命令",
            provides: vec![EXEC.id()],
            requires: vec![],
            optional_requires: vec![],
        }
    }

    fn start(&self, config: &ConfigSection, host: &mut Host<'_>) -> Result<(), AppError> {
        config.parse::<NoConfig>()?;
        host.provide(&EXEC, Arc::new(Runner))
    }
}

register_component!(ExecTokio);

#[cfg(test)]
mod tests {
    use exec_contract::testing::exec_contract;

    use super::*;

    #[tokio::test]
    async fn passes_the_exec_contract() {
        let app = testkit::app().with(ExecTokio).boot().await.unwrap();
        exec_contract(app.resolve(&EXEC).unwrap()).await.unwrap();
        app.stop().await;
    }
}
