//! 契约：运行一个外部命令，给它 stdin 和环境变量，拿回退出码和输出。
//!
//! 给「用户在配置里写的命令」用：应用功能自己不起进程，通过这个契约起。

use kernel_api::{AppError, contract};
use serde::{Deserialize, Serialize};

/// 要运行的命令。
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct CommandRequest {
    /// 程序和参数。不经过 shell。
    pub argv: Vec<String>,
    /// 写进 stdin 的内容；`None` 时 stdin 是空的。
    #[serde(default)]
    pub stdin: Option<String>,
    /// 额外的环境变量（在继承的环境之上）。
    #[serde(default)]
    pub env: Vec<(String, String)>,
    /// 最多跑多久，到时间就杀掉。
    pub timeout_ms: u64,
}

/// 跑完的结果。退出码不是 0 也是结果，不是错误：怎么算失败由调用方决定。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CommandOutput {
    /// 退出码；被信号杀掉是 `None`。
    pub code: Option<i32>,
    /// 按 UTF-8 解码（坏的字节换成替换字符）。
    pub stdout: String,
    pub stderr: String,
}

contract! {
    /// 外部命令。
    pub trait Exec as EXEC = service("exec", 1) {
        /// 运行到结束。`argv` 是空的是 `validation`；程序起不来是 `unavailable`；
        /// 超时被杀是 `temp_fail`。
        async fn run(&self, request: CommandRequest) -> Result<CommandOutput, AppError>;
    }
}

#[cfg(feature = "testing")]
pub mod testing {
    //! 契约测试（用 `/bin/sh`）和假实现。

    use std::collections::VecDeque;
    use std::sync::{Arc, Mutex, PoisonError};

    use kernel_api::{
        AppError, Component, ConfigSection, ErrorKind, Host, Kind, Manifest, async_trait,
    };

    use super::{CommandOutput, CommandRequest, EXEC, Exec};

    fn check(ok: bool, what: &str) -> Result<(), AppError> {
        if ok {
            Ok(())
        } else {
            Err(AppError::validation(format!("契约不满足：{what}")))
        }
    }

    fn sh(script: &str) -> CommandRequest {
        CommandRequest {
            argv: vec!["/bin/sh".into(), "-c".into(), script.into()],
            timeout_ms: 5_000,
            ..CommandRequest::default()
        }
    }

    /// 任何实现都要通过。
    pub async fn exec_contract(exec: Arc<dyn Exec>) -> Result<(), AppError> {
        let out = exec
            .run(CommandRequest {
                stdin: Some("你好".into()),
                env: vec![("STASH_TEST_VAR".into(), "值".into())],
                ..sh("cat; printf ' %s' \"$STASH_TEST_VAR\"; echo oops >&2; exit 3")
            })
            .await?;
        check(out.stdout == "你好 值", "stdin 和环境变量传进去了")?;
        check(out.stderr == "oops\n", "stderr 分开")?;
        check(out.code == Some(3), "退出码不是 0 也是结果")?;
        let out = exec.run(sh("exit 0")).await?;
        check(
            out.code == Some(0) && out.stdout.is_empty(),
            "没有 stdin 时是空的",
        )?;

        let started = std::time::Instant::now();
        let err = exec
            .run(CommandRequest {
                timeout_ms: 100,
                ..sh("sleep 5")
            })
            .await;
        check(
            matches!(err, Err(ref e) if e.kind == ErrorKind::TempFail),
            "超时是 temp_fail",
        )?;
        check(started.elapsed().as_secs() < 3, "超时就杀掉，不等它跑完")?;

        let err = exec
            .run(CommandRequest {
                argv: vec!["/no/such/program".into()],
                timeout_ms: 1_000,
                ..CommandRequest::default()
            })
            .await;
        check(
            matches!(err, Err(ref e) if e.kind == ErrorKind::Unavailable),
            "起不来是 unavailable",
        )?;
        let err = exec.run(CommandRequest::default()).await;
        check(
            matches!(err, Err(ref e) if e.kind == ErrorKind::Validation),
            "argv 为空是 validation",
        )
    }

    /// 假实现：按顺序给出排好的结果，并记下收到的请求。排的用完了返回 `unavailable`。
    /// 克隆出来的是同一个。
    #[derive(Clone, Default)]
    pub struct FakeExec {
        script: Arc<Mutex<VecDeque<Result<CommandOutput, AppError>>>>,
        seen: Arc<Mutex<Vec<CommandRequest>>>,
    }

    impl FakeExec {
        pub fn new(results: impl IntoIterator<Item = Result<CommandOutput, AppError>>) -> Self {
            Self {
                script: Arc::new(Mutex::new(results.into_iter().collect())),
                seen: Arc::default(),
            }
        }

        /// 成功退出、输出这些。
        pub fn ok(stdout: &str) -> Result<CommandOutput, AppError> {
            Ok(CommandOutput {
                code: Some(0),
                stdout: stdout.to_owned(),
                stderr: String::new(),
            })
        }

        pub fn seen(&self) -> Vec<CommandRequest> {
            self.seen
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .clone()
        }
    }

    #[async_trait]
    impl Exec for FakeExec {
        async fn run(&self, request: CommandRequest) -> Result<CommandOutput, AppError> {
            self.seen
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .push(request);
            self.script
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .pop_front()
                .unwrap_or_else(|| Err(AppError::unavailable("FakeExec 没有排好的结果了")))
        }
    }

    /// 组件 `exec-fake`。
    impl Component for FakeExec {
        fn manifest(&self) -> Manifest {
            Manifest {
                id: "exec-fake",
                kind: Kind::Service,
                summary: "测试用：给出排好的结果的假外部命令",
                provides: vec![EXEC.id()],
                requires: vec![],
                optional_requires: vec![],
            }
        }

        fn start(&self, _config: &ConfigSection, host: &mut Host<'_>) -> Result<(), AppError> {
            host.provide(&EXEC, Arc::new(self.clone()))
        }
    }
}
