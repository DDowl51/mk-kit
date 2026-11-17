//! 契约：按目录查它所在的版本库。
//!
//! 描述的是能力，不是某个版本控制工具：这里没有任何工具的名字。

use kernel_api::{AppError, contract};
use serde::{Deserialize, Serialize};

/// 一个版本库此刻的状态。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RepoInfo {
    /// 版本库的根目录，绝对路径。
    pub root: String,
    /// 当前分支；不在分支上（比如检出了某个提交）是 `None`。
    #[serde(default)]
    pub branch: Option<String>,
    /// 当前提交的短 id；还没有任何提交是 `None`。
    #[serde(default)]
    pub commit: Option<String>,
    /// 主远端的地址；没有远端是 `None`。
    #[serde(default)]
    pub remote_url: Option<String>,
    /// 有没有没提交的改动（不算没被跟踪的新文件）。
    pub dirty: bool,
}

contract! {
    /// 版本库。
    pub trait Vcs as VCS = service("vcs", 1) {
        /// `path` 所在的版本库。不在任何版本库里（包括路径不存在）是 `None`。
        /// 版本控制工具本身用不了是 `unavailable`。
        async fn repo_at(&self, path: String) -> Result<Option<RepoInfo>, AppError>;
    }
}

#[cfg(feature = "testing")]
pub mod testing {
    //! 契约测试。版本库由调用方按实现的工具事先建好。

    use std::sync::Arc;

    use kernel_api::AppError;

    use super::Vcs;

    fn check(ok: bool, what: &str) -> Result<(), AppError> {
        if ok {
            Ok(())
        } else {
            Err(AppError::validation(format!("契约不满足：{what}")))
        }
    }

    /// 调用方准备好的目录。
    pub struct Fixture {
        /// 一个版本库：在分支 `main` 上、至少一个提交、主远端是 `https://example.com/r.git`、没有改动。
        pub repo: String,
        /// `repo` 里的一个子目录。
        pub sub: String,
        /// 和 `repo` 一样，但有一个已跟踪文件改过了。
        pub dirty_repo: String,
        /// 不在任何版本库里的目录。
        pub plain: String,
    }

    /// 任何实现都要通过。
    pub async fn vcs_contract(vcs: Arc<dyn Vcs>, f: &Fixture) -> Result<(), AppError> {
        let repo = vcs
            .repo_at(f.sub.clone())
            .await?
            .ok_or_else(|| AppError::validation("契约不满足：子目录也算在版本库里"))?;
        check(repo.root == f.repo, "根目录是版本库的根")?;
        check(repo.branch.as_deref() == Some("main"), "当前分支")?;
        check(
            repo.commit.as_deref().is_some_and(|c| !c.is_empty()),
            "当前提交",
        )?;
        check(
            repo.remote_url.as_deref() == Some("https://example.com/r.git"),
            "主远端",
        )?;
        check(!repo.dirty, "没改动")?;
        let dirty = vcs.repo_at(f.dirty_repo.clone()).await?;
        check(dirty.is_some_and(|r| r.dirty), "有改动")?;
        check(vcs.repo_at(f.plain.clone()).await?.is_none(), "不是版本库")?;
        check(
            vcs.repo_at(format!("{}/no/such/dir", f.plain))
                .await?
                .is_none(),
            "路径不存在",
        )
    }
}
