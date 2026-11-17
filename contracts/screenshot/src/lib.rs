//! 契约：截一个窗口的图，写成 PNG 文件。
//!
//! 描述的是能力，不是某个合成器：这里没有任何合成器的名字。
//! 截出来的文件由这个服务负责删除（[`Screenshots::remove`]），使用方不用再依赖文件服务。

use kernel_api::{AppError, contract};

contract! {
    /// 窗口截图。
    pub trait Screenshots as SCREENSHOTS = service("screenshot", 1) {
        /// 把窗口 `window_id`（窗口管理器给的编号）截成 PNG 写到 `path`（绝对路径，没有的目录会建好）。
        /// 返回时文件已经写完。窗口不存在是 `not_found`；路径不是绝对路径是 `validation`。
        /// 不能有别的可见副作用（比如改动剪贴板）。
        async fn window(&self, window_id: u64, path: String) -> Result<(), AppError>;

        /// 删掉之前截的图。本来就不存在返回 `false`。
        async fn remove(&self, path: String) -> Result<bool, AppError>;
    }
}

#[cfg(feature = "testing")]
pub mod testing {
    //! 契约测试和假实现。

    use std::collections::BTreeMap;
    use std::sync::{Arc, Mutex, PoisonError};

    use kernel_api::{
        AppError, Component, ConfigSection, ErrorKind, Host, Kind, Manifest, async_trait,
    };

    use super::{SCREENSHOTS, Screenshots};

    /// PNG 文件开头的 8 个字节。
    pub const PNG_SIGNATURE: [u8; 8] = [0x89, b'P', b'N', b'G', b'\r', b'\n', 0x1a, b'\n'];

    fn check(ok: bool, what: &str) -> Result<(), AppError> {
        if ok {
            Ok(())
        } else {
            Err(AppError::validation(format!("契约不满足：{what}")))
        }
    }

    /// `window_id` 是一个存在的窗口，`missing_id` 是不存在的；`dir` 是一个可写的空目录（绝对路径）。
    pub async fn screenshot_contract(
        shots: Arc<dyn Screenshots>,
        window_id: u64,
        missing_id: u64,
        dir: &str,
    ) -> Result<(), AppError> {
        let path = format!("{dir}/sub/shot.png");
        shots.window(window_id, path.clone()).await?;
        let bytes = std::fs::read(&path)
            .map_err(|e| AppError::validation(format!("契约不满足：文件写出来了（{e}）")))?;
        check(bytes.starts_with(&PNG_SIGNATURE), "写出的是 PNG")?;
        let err = shots.window(missing_id, format!("{dir}/missing.png")).await;
        check(
            matches!(err, Err(ref e) if e.kind == ErrorKind::NotFound),
            "不存在的窗口是 not_found",
        )?;
        let err = shots.window(window_id, "relative.png".to_owned()).await;
        check(
            matches!(err, Err(ref e) if e.kind == ErrorKind::Validation),
            "相对路径是 validation",
        )?;
        check(shots.remove(path.clone()).await?, "删掉了")?;
        check(!std::path::Path::new(&path).exists(), "文件没了")?;
        check(!shots.remove(path).await?, "再删一次返回 false")
    }

    /// 假截图：不碰文件系统，记下「截过的」路径。克隆出来的是同一个。
    #[derive(Clone, Default)]
    pub struct FakeScreenshots {
        /// 路径 → 窗口编号。
        taken: Arc<Mutex<BTreeMap<String, u64>>>,
        /// 存在的窗口；空的表示都存在。
        windows: Arc<Mutex<Vec<u64>>>,
    }

    impl FakeScreenshots {
        /// 只有这些窗口存在。
        pub fn with_windows(ids: &[u64]) -> Self {
            let fake = Self::default();
            *fake.windows.lock().unwrap_or_else(PoisonError::into_inner) = ids.to_vec();
            fake
        }

        /// 现在还留着的截图（路径 → 窗口编号）。
        pub fn taken(&self) -> BTreeMap<String, u64> {
            self.taken
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .clone()
        }
    }

    #[async_trait]
    impl Screenshots for FakeScreenshots {
        async fn window(&self, window_id: u64, path: String) -> Result<(), AppError> {
            if !path.starts_with('/') {
                return Err(AppError::validation(format!("`{path}` 不是绝对路径")));
            }
            let windows = self
                .windows
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .clone();
            if !windows.is_empty() && !windows.contains(&window_id) {
                return Err(AppError::not_found(format!("没有窗口 {window_id}")));
            }
            self.taken
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .insert(path, window_id);
            Ok(())
        }

        async fn remove(&self, path: String) -> Result<bool, AppError> {
            Ok(self
                .taken
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .remove(&path)
                .is_some())
        }
    }

    /// 组件 `screenshot-fake`。
    impl Component for FakeScreenshots {
        fn manifest(&self) -> Manifest {
            Manifest {
                id: "screenshot-fake",
                kind: Kind::Service,
                summary: "测试用：只记账的假截图",
                provides: vec![SCREENSHOTS.id()],
                requires: vec![],
                optional_requires: vec![],
            }
        }

        fn start(&self, _config: &ConfigSection, host: &mut Host<'_>) -> Result<(), AppError> {
            host.provide(&SCREENSHOTS, Arc::new(self.clone()))
        }
    }
}
