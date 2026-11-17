//! 系统服务 `vcs-git`：调 `git` 命令，提供 `Vcs`。
//!
//! 用命令而不是库：和用户手里的 git 版本、配置（safe.directory 之类）行为一致。

use std::process::Stdio;
use std::sync::Arc;

use kernel_api::{
    AppError, Component, ConfigSection, Host, Kind, Manifest, async_trait, register_component,
};
use serde::{Deserialize, Serialize};
use tokio::process::Command;
use vcs_contract::{RepoInfo, VCS, Vcs};

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields, default)]
struct Config {
    /// git 的命令。
    command: String,
    /// 主远端的名字。
    remote: String,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            command: "git".to_owned(),
            remote: "origin".to_owned(),
        }
    }
}

struct Git(Config);

/// 一次 git 调用的结果：成功时的 stdout（去掉首尾空白），失败是 `None`。
type Answer = Option<String>;

impl Git {
    async fn git(&self, dir: &str, args: &[&str]) -> Result<Answer, AppError> {
        let out = Command::new(&self.0.command)
            .arg("-C")
            .arg(dir)
            .args(args)
            // 不要弹出输入密码之类的交互
            .env("GIT_TERMINAL_PROMPT", "0")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true)
            .output()
            .await
            .map_err(|e| AppError::unavailable(format!("运行 `{}` 失败：{e}", self.0.command)))?;
        Ok(out
            .status
            .success()
            .then(|| String::from_utf8_lossy(&out.stdout).trim().to_owned()))
    }
}

#[async_trait]
impl Vcs for Git {
    async fn repo_at(&self, path: String) -> Result<Option<RepoInfo>, AppError> {
        if !std::path::Path::new(&path).is_dir() {
            return Ok(None);
        }
        let Some(root) = self.git(&path, &["rev-parse", "--show-toplevel"]).await? else {
            return Ok(None);
        };
        let remote_key = format!("remote.{}.url", self.0.remote);
        let remote_args = ["config", "--get", remote_key.as_str()];
        // 互不依赖，一起问
        let (branch, commit, remote, status) = tokio::try_join!(
            self.git(&root, &["symbolic-ref", "--short", "-q", "HEAD"]),
            self.git(&root, &["rev-parse", "--short", "-q", "--verify", "HEAD"]),
            self.git(&root, &remote_args),
            self.git(&root, &["status", "--porcelain", "--untracked-files=no"]),
        )?;
        let some = |a: Answer| a.filter(|s| !s.is_empty());
        Ok(Some(RepoInfo {
            root,
            branch: some(branch),
            commit: some(commit),
            remote_url: some(remote),
            dirty: some(status).is_some(),
        }))
    }
}

pub struct VcsGit;

impl Component for VcsGit {
    fn config_defaults(&self) -> Option<ConfigSection> {
        ConfigSection::defaults::<Config>()
    }

    fn manifest(&self) -> Manifest {
        Manifest {
            id: "vcs-git",
            kind: Kind::Service,
            summary: "调 git：按目录查仓库、分支、提交、远端",
            provides: vec![VCS.id()],
            requires: vec![],
            optional_requires: vec![],
        }
    }

    fn start(&self, config: &ConfigSection, host: &mut Host<'_>) -> Result<(), AppError> {
        let config: Config = config.parse()?;
        if config.command.trim().is_empty() || config.remote.trim().is_empty() {
            return Err(AppError::config("vcs-git 的 command、remote 不能是空的"));
        }
        host.provide(&VCS, Arc::new(Git(config)))
    }
}

register_component!(VcsGit);

#[cfg(test)]
mod tests {
    use std::path::Path;

    use vcs_contract::testing::{Fixture, vcs_contract};

    use super::*;

    /// 不读用户的 git 配置，身份也是假的。
    fn git(dir: &Path, args: &[&str]) {
        let status = std::process::Command::new("git")
            .arg("-C")
            .arg(dir)
            .args([
                "-c",
                "user.name=test",
                "-c",
                "user.email=test@example.invalid",
                "-c",
                "commit.gpgsign=false",
                "-c",
                "init.defaultBranch=main",
            ])
            .args(args)
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .unwrap();
        assert!(status.success(), "git {args:?}");
    }

    fn repo(dir: &Path) {
        std::fs::create_dir_all(dir.join("src")).unwrap();
        std::fs::write(dir.join("src/a.txt"), "a").unwrap();
        git(dir, &["init", "-q"]);
        git(
            dir,
            &["remote", "add", "origin", "https://example.com/r.git"],
        );
        git(dir, &["add", "."]);
        git(dir, &["commit", "-q", "-m", "init"]);
    }

    fn s(p: &Path) -> String {
        // macOS 之类的 /tmp 是符号链接；git 给的是真实路径
        p.canonicalize().unwrap().to_string_lossy().into_owned()
    }

    #[tokio::test]
    async fn passes_the_vcs_contract() {
        let tmp = tempfile::tempdir().unwrap();
        let (clean, dirty, plain) = (
            tmp.path().join("r"),
            tmp.path().join("d"),
            tmp.path().join("p"),
        );
        repo(&clean);
        repo(&dirty);
        std::fs::write(dirty.join("src/a.txt"), "changed").unwrap();
        std::fs::create_dir_all(&plain).unwrap();
        let app = testkit::app().with(VcsGit).boot().await.unwrap();
        vcs_contract(
            app.resolve(&VCS).unwrap(),
            &Fixture {
                repo: s(&clean),
                sub: s(&clean.join("src")),
                dirty_repo: s(&dirty),
                plain: s(&plain),
            },
        )
        .await
        .unwrap();
        app.stop().await;
    }

    #[tokio::test]
    async fn detached_and_empty_repos() {
        let tmp = tempfile::tempdir().unwrap();
        let empty = tmp.path().join("e");
        std::fs::create_dir_all(&empty).unwrap();
        git(&empty, &["init", "-q"]);
        let got = Git(Config::default())
            .repo_at(s(&empty))
            .await
            .unwrap()
            .unwrap();
        assert_eq!((got.commit, got.remote_url, got.dirty), (None, None, false));

        let r = tmp.path().join("r");
        repo(&r);
        git(&r, &["checkout", "-q", "--detach"]);
        let got = Git(Config::default())
            .repo_at(s(&r))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(got.branch, None);
        assert!(got.commit.is_some());
    }

    #[tokio::test]
    async fn a_missing_git_is_unavailable() {
        let tmp = tempfile::tempdir().unwrap();
        let err = Git(Config {
            command: "/no/such/git".to_owned(),
            ..Config::default()
        })
        .repo_at(s(tmp.path()))
        .await
        .unwrap_err();
        assert_eq!(err.kind, kernel_api::ErrorKind::Unavailable);
    }
}
