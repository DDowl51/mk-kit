//! 契约：弹一个输入框，要一行文字。
//!
//! 描述的是能力，不是某个启动器或菜单程序：这里没有任何具体程序的名字。
//! 换输入框 = 写另一个提供这个契约的系统服务。

use kernel_api::{AppError, contract};
use serde::{Deserialize, Serialize};

/// 要弹的输入框。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PromptRequest {
    /// 输入框里什么都没写时显示的提示。
    #[serde(default)]
    pub placeholder: String,
    /// 预先填好的文字。
    #[serde(default)]
    pub initial: String,
}

/// 用户的回答。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Answer {
    /// 写了字并确认。已经去掉首尾空白，不会是空的。
    Text(String),
    /// 取消了，或者什么都没写就确认。
    Cancelled,
}

impl Answer {
    /// 把输入框给回来的原文按契约整理：去掉首尾空白，空的算取消。实现方共用。
    pub fn from_raw(raw: &str) -> Self {
        let text = raw.trim();
        if text.is_empty() {
            Self::Cancelled
        } else {
            Self::Text(text.to_owned())
        }
    }
}

contract! {
    /// 输入框。
    pub trait Prompt as PROMPT = service("prompt", 1) {
        /// 弹出输入框，等用户写完或取消。没有超时：用户想写多久就写多久。
        /// 输入框程序起不来是 `unavailable`。
        async fn ask(&self, request: PromptRequest) -> Result<Answer, AppError>;
    }
}

#[cfg(any(test, feature = "testing"))]
pub mod testing {
    //! 契约测试和假输入框。

    use std::collections::VecDeque;
    use std::sync::{Arc, Mutex, PoisonError};

    use kernel_api::{AppError, Component, ConfigSection, Host, Kind, Manifest, async_trait};

    use super::{Answer, PROMPT, Prompt, PromptRequest};

    /// 用户在输入框里做了什么。
    #[derive(Debug, Clone, PartialEq, Eq)]
    pub enum Scenario {
        /// 写了这些字，按回车。
        Types(String),
        /// 什么都没写就按回车。
        EntersNothing,
        /// 按 Esc。
        Escapes,
    }

    fn check(ok: bool, what: &str) -> Result<(), AppError> {
        if ok {
            Ok(())
        } else {
            Err(AppError::validation(format!("契约不满足：{what}")))
        }
    }

    /// 契约测试：`make` 给出一个「用户会这样操作」的输入框。任何实现都要通过。
    pub async fn prompt_contract<F>(make: F) -> Result<(), AppError>
    where
        F: Fn(Scenario) -> Arc<dyn Prompt>,
    {
        let request = PromptRequest {
            placeholder: "记点什么".to_owned(),
            initial: String::new(),
        };
        let answer = make(Scenario::Types("  回复老王 #t ".to_owned()))
            .ask(request.clone())
            .await?;
        check(
            answer == Answer::Text("回复老王 #t".to_owned()),
            "写的字原样返回，只去掉首尾空白",
        )?;
        let answer = make(Scenario::EntersNothing).ask(request.clone()).await?;
        check(answer == Answer::Cancelled, "什么都没写就回车算取消")?;
        let answer = make(Scenario::Types("   ".to_owned()))
            .ask(request.clone())
            .await?;
        check(answer == Answer::Cancelled, "只写了空白算取消")?;
        let answer = make(Scenario::Escapes).ask(request).await?;
        check(answer == Answer::Cancelled, "Esc 是取消")
    }

    /// 假输入框：按排好的顺序回答，并记下每次弹框的请求。排的用完了就当用户按了 Esc。
    /// 克隆出来的是同一个。
    #[derive(Clone, Default)]
    pub struct FakePrompt {
        script: Arc<Mutex<VecDeque<Scenario>>>,
        asked: Arc<Mutex<Vec<PromptRequest>>>,
    }

    impl FakePrompt {
        pub fn new(script: impl IntoIterator<Item = Scenario>) -> Self {
            Self {
                script: Arc::new(Mutex::new(script.into_iter().collect())),
                asked: Arc::default(),
            }
        }

        /// 用户会写这些字并回车。
        pub fn types(text: &str) -> Self {
            Self::new([Scenario::Types(text.to_owned())])
        }

        /// 弹过的输入框。
        pub fn asked(&self) -> Vec<PromptRequest> {
            self.asked
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .clone()
        }
    }

    #[async_trait]
    impl Prompt for FakePrompt {
        async fn ask(&self, request: PromptRequest) -> Result<Answer, AppError> {
            self.asked
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .push(request);
            let next = self
                .script
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .pop_front();
            Ok(match next {
                Some(Scenario::Types(text)) => Answer::from_raw(&text),
                Some(Scenario::EntersNothing | Scenario::Escapes) | None => Answer::Cancelled,
            })
        }
    }

    /// 组件 `prompt-fake`。
    impl Component for FakePrompt {
        fn manifest(&self) -> Manifest {
            Manifest {
                id: "prompt-fake",
                kind: Kind::Service,
                summary: "测试用：按排好的顺序回答的假输入框",
                provides: vec![PROMPT.id()],
                requires: vec![],
                optional_requires: vec![],
            }
        }

        fn start(&self, _config: &ConfigSection, host: &mut Host<'_>) -> Result<(), AppError> {
            host.provide(&PROMPT, Arc::new(self.clone()))
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[tokio::test]
        async fn the_fake_passes_the_contract() {
            prompt_contract(|s| Arc::new(FakePrompt::new([s])) as Arc<dyn Prompt>)
                .await
                .unwrap();
        }
    }
}
