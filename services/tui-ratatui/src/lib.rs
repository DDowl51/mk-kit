//! 系统服务 `tui-ratatui`：用 ratatui 画终端界面。
//!
//! 它不认识任何页面：页面由各组件登记到 `TUI_PAGES`，内容是纯数据。
//! 终端库只出现在这个包里，换一个库 = 换这个组件。

mod app;
mod draw;

use std::io::IsTerminal;
use std::sync::Arc;
use std::time::{Duration, Instant};

use kernel_api::{
    AppError, Collector, Component, ConfigSection, Host, Kind, Manifest, async_trait,
    register_component,
};
use ratatui::crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};
use serde::{Deserialize, Serialize};
use tui_contract::{ActionRequest, PageProvider, PageRequest, PageSpec, TUI, TUI_PAGES, Tui};

use crate::app::{App, Effect, Key, RESERVED};

#[derive(Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields, default)]
struct Config {
    /// 停在「现在」那一页时，每隔多少秒自动刷新。0 表示不自动刷新。
    refresh_secs: u64,
    /// 页面要求打开网址或文件时用的命令，网址或路径加在最后。
    opener: Vec<String>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            refresh_secs: 30,
            opener: vec!["xdg-open".to_owned()],
        }
    }
}

fn key_of(event: &event::KeyEvent) -> Key {
    if event.modifiers.contains(KeyModifiers::CONTROL) && event.code == KeyCode::Char('c') {
        return Key::Interrupt;
    }
    match event.code {
        KeyCode::Char(c) => Key::Char(c),
        KeyCode::Left => Key::Left,
        KeyCode::Right => Key::Right,
        KeyCode::Up => Key::Up,
        KeyCode::Down => Key::Down,
        KeyCode::PageUp => Key::PageUp,
        KeyCode::PageDown => Key::PageDown,
        KeyCode::Home => Key::Home,
        KeyCode::Tab => Key::Tab,
        KeyCode::BackTab => Key::BackTab,
        KeyCode::Esc => Key::Esc,
        KeyCode::Enter => Key::Enter,
        KeyCode::Backspace => Key::Backspace,
        KeyCode::F(5) => Key::Refresh,
        _ => Key::Other,
    }
}

fn io(e: &std::io::Error) -> AppError {
    AppError::unavailable(format!("终端出错：{e}"))
}

struct Service {
    providers: Collector<dyn PageProvider>,
    refresh: Option<Duration>,
    opener: Vec<String>,
}

/// 用配置的命令打开，不等它结束。打不开时返回给用户看的原因。
fn open(opener: &[String], target: &str) -> Result<(), String> {
    let Some((program, args)) = opener.split_first() else {
        return Err("没有配置打开用的命令（tui-ratatui 的 opener）".to_owned());
    };
    let spawned = tokio::process::Command::new(program)
        .args(args)
        .arg(target)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn();
    match spawned {
        Ok(mut child) => {
            // 收尸，不然会留下僵尸进程
            tokio::spawn(async move {
                let _ = child.wait().await;
            });
            Ok(())
        }
        Err(e) => Err(format!("打不开（{program}）：{e}")),
    }
}

impl Service {
    /// 收集所有页面，记下各自由哪个组件提供。两个页面用同一个键是装配错误。
    async fn pages(&self) -> Result<Vec<(Arc<dyn PageProvider>, PageSpec)>, AppError> {
        let mut all = Vec::new();
        for provider in self.providers.all() {
            for spec in provider.pages().await? {
                all.push((provider.clone(), spec));
            }
        }
        all.sort_by_key(|(_, spec)| spec.order);
        for (i, (_, spec)) in all.iter().enumerate() {
            if let Some((_, other)) = all[..i].iter().find(|(_, o)| o.key == spec.key) {
                return Err(AppError::config(format!(
                    "页面「{}」和「{}」都用了键 `{}`",
                    other.title, spec.title, spec.key
                )));
            }
        }
        // 页面的按键不能和外壳的键、切页的键重复，也不能自己重复
        for (_, spec) in &all {
            for (j, action) in spec.actions.iter().enumerate() {
                let clash = if RESERVED.contains(&action.key) {
                    Some("外壳自己的键".to_owned())
                } else if let Some((_, page)) = all.iter().find(|(_, p)| p.key == action.key) {
                    Some(format!("页面「{}」的切换键", page.title))
                } else if spec.actions[..j].iter().any(|a| a.key == action.key) {
                    Some("同一页的另一个按键".to_owned())
                } else {
                    None
                };
                if let Some(clash) = clash {
                    return Err(AppError::config(format!(
                        "页面「{}」的按键 `{}`（{}）和{clash}重复了",
                        spec.title, action.key, action.label
                    )));
                }
            }
        }
        Ok(all)
    }

    async fn event_loop(
        &self,
        terminal: &mut ratatui::DefaultTerminal,
        pages: &[(Arc<dyn PageProvider>, PageSpec)],
    ) -> Result<(), AppError> {
        let mut app = App::new(pages.iter().map(|(_, spec)| spec.clone()).collect());
        let mut reload = true;
        let mut loaded_at = Instant::now();
        loop {
            if reload && let Some((provider, spec)) = pages.get(app.current) {
                let width = terminal.size().map_err(|e| io(&e))?.width;
                let request = PageRequest {
                    offset: app.offset,
                    width,
                    input: app.input().to_owned(),
                };
                let view = provider
                    .render(spec.id.clone(), request)
                    .await
                    .map_err(|e| e.message);
                app.loaded(view);
                loaded_at = Instant::now();
            }
            reload = false;
            terminal
                .draw(|frame| draw::draw(frame, &app))
                .map_err(|e| io(&e))?;

            // 读按键是阻塞的，挪到阻塞线程上；每秒醒一次，看要不要自动刷新
            let next = tokio::task::spawn_blocking(|| -> std::io::Result<Option<Event>> {
                if event::poll(Duration::from_secs(1))? {
                    event::read().map(Some)
                } else {
                    Ok(None)
                }
            })
            .await
            .map_err(|e| AppError::unavailable(format!("读按键的线程出错：{e}")))?
            .map_err(|e| io(&e))?;

            match next {
                None => {
                    reload = app.offset == 0
                        && self
                            .refresh
                            .is_some_and(|every| loaded_at.elapsed() >= every);
                }
                Some(Event::Key(key)) if key.kind == KeyEventKind::Press => {
                    match app.handle(key_of(&key)) {
                        Effect::Quit => return Ok(()),
                        Effect::Reload => reload = true,
                        Effect::Act(request) => reload = self.act(&mut app, pages, request).await,
                        Effect::Redraw | Effect::Nothing => {}
                    }
                }
                // 宽度变了，页面可能要重新排
                Some(Event::Resize(..)) => reload = true,
                Some(_) => {}
            }
        }
    }
}

impl Service {
    /// 把按键交给当前页面，执行它的要求。返回要不要重取内容。
    async fn act(
        &self,
        app: &mut App,
        pages: &[(Arc<dyn PageProvider>, PageSpec)],
        request: ActionRequest,
    ) -> bool {
        let Some((provider, spec)) = pages.get(app.current) else {
            return false;
        };
        let (key, row) = (request.key, request.row.clone());
        match provider.act(spec.id.clone(), request).await {
            Ok(result) => {
                let (effect, target) = app.apply(key, row, result);
                if let Some(target) = target
                    && let Err(why) = open(&self.opener, &target)
                {
                    app.message = Some(why);
                }
                effect == Effect::Reload
            }
            // 页面出错不退出：显示出来，用户可以接着操作
            Err(e) => {
                app.message = Some(format!("出错了：{}", e.message));
                false
            }
        }
    }
}

#[async_trait]
impl Tui for Service {
    async fn run(&self) -> Result<u8, AppError> {
        if !std::io::stdout().is_terminal() {
            return Err(AppError::validation(
                "终端界面要在终端里运行：stdout 不是终端",
            ));
        }
        let pages = self.pages().await?;
        // 会装上 panic 时恢复终端的钩子
        let mut terminal = ratatui::try_init().map_err(|e| io(&e))?;
        let result = self.event_loop(&mut terminal, &pages).await;
        let restored = ratatui::try_restore().map_err(|e| io(&e));
        result.and(restored).map(|()| 0)
    }
}

pub struct TuiRatatui;

impl Component for TuiRatatui {
    fn config_defaults(&self) -> Option<ConfigSection> {
        ConfigSection::defaults::<Config>()
    }

    fn manifest(&self) -> Manifest {
        Manifest {
            id: "tui-ratatui",
            kind: Kind::Service,
            summary: "画终端界面，页面由各组件登记",
            provides: vec![TUI.id(), TUI_PAGES.id()],
            requires: vec![],
            optional_requires: vec![],
        }
    }

    fn start(&self, config: &ConfigSection, host: &mut Host<'_>) -> Result<(), AppError> {
        let config: Config = config.parse()?;
        let refresh = (config.refresh_secs > 0).then(|| Duration::from_secs(config.refresh_secs));
        let providers = host.collector(&TUI_PAGES)?;
        host.provide(
            &TUI,
            Arc::new(Service {
                providers,
                refresh,
                opener: config.opener,
            }),
        )
    }
}

register_component!(TuiRatatui);

#[cfg(test)]
mod tests {
    use kernel_api::ErrorKind;
    use tui_contract::{ActionResult, PageView};

    use super::*;

    struct Pages(Vec<(char, i32)>, Vec<char>);

    #[async_trait]
    impl PageProvider for Pages {
        async fn pages(&self) -> Result<Vec<PageSpec>, AppError> {
            Ok(self
                .0
                .iter()
                .map(|(key, order)| PageSpec {
                    id: key.to_string(),
                    title: format!("页{key}"),
                    key: *key,
                    order: *order,
                    input: None,
                    actions: self
                        .1
                        .iter()
                        .map(|k| tui_contract::ActionSpec {
                            key: *k,
                            label: format!("键{k}"),
                            confirm: false,
                        })
                        .collect(),
                })
                .collect())
        }

        async fn render(&self, page: String, _: PageRequest) -> Result<PageView, AppError> {
            Ok(PageView {
                heading: page,
                note: String::new(),
                blocks: vec![],
            })
        }

        async fn act(&self, _: String, _: ActionRequest) -> Result<ActionResult, AppError> {
            Ok(ActionResult::default())
        }
    }

    struct Owner(Vec<(char, i32)>, Vec<char>);

    impl Component for Owner {
        fn manifest(&self) -> Manifest {
            Manifest {
                id: "owner",
                kind: Kind::Feature,
                summary: "测试用",
                provides: vec![],
                requires: vec![TUI_PAGES.id()],
                optional_requires: vec![],
            }
        }

        fn start(&self, _: &ConfigSection, host: &mut Host<'_>) -> Result<(), AppError> {
            host.contribute(&TUI_PAGES, Arc::new(Pages(self.0.clone(), self.1.clone())))
        }
    }

    async fn service(pages: Vec<(char, i32)>) -> (testkit::TestApp, Service) {
        service_with(pages, vec![]).await
    }

    async fn service_with(
        pages: Vec<(char, i32)>,
        actions: Vec<char>,
    ) -> (testkit::TestApp, Service) {
        // 扩展点只有所有者能取回，所以借用一个真的启动过程，再单独造一个同样接线的 Service
        struct Probe(Arc<std::sync::OnceLock<Collector<dyn PageProvider>>>);
        impl Component for Probe {
            fn manifest(&self) -> Manifest {
                TuiRatatui.manifest()
            }
            fn start(&self, _: &ConfigSection, host: &mut Host<'_>) -> Result<(), AppError> {
                let _ = self.0.set(host.collector(&TUI_PAGES)?);
                Ok(())
            }
        }
        let slot = Arc::new(std::sync::OnceLock::new());
        let app = testkit::app()
            .with(Probe(slot.clone()))
            .with(Owner(pages, actions))
            .boot()
            .await
            .unwrap();
        let providers = slot.get().unwrap().clone();
        (
            app,
            Service {
                providers,
                refresh: None,
                opener: vec![],
            },
        )
    }

    #[tokio::test]
    async fn pages_are_ordered() {
        let (_app, service) = service(vec![('w', 30), ('d', 10), ('t', 20)]).await;
        let keys: Vec<char> = service
            .pages()
            .await
            .unwrap()
            .into_iter()
            .map(|(_, s)| s.key)
            .collect();
        assert_eq!(keys, ['d', 't', 'w']);
    }

    #[tokio::test]
    async fn two_pages_with_the_same_key_is_an_error() {
        let (_app, service) = service(vec![('d', 1), ('d', 2)]).await;
        let err = service.pages().await.err().unwrap();
        assert_eq!(err.kind, ErrorKind::Config);
        assert!(err.message.contains("`d`"), "{err}");
    }

    #[tokio::test]
    async fn page_keys_must_not_clash() {
        let (_app, ok) = service_with(vec![('1', 1), ('2', 2)], vec!['x', 'r']).await;
        ok.pages().await.unwrap();
        for (keys, clash) in [
            (vec!['q'], "外壳自己的键"),
            (vec!['j'], "外壳自己的键"),
            (vec!['2'], "切换键"),
            (vec!['x', 'x'], "同一页"),
        ] {
            let (_app, service) = service_with(vec![('1', 1), ('2', 2)], keys.clone()).await;
            let err = service.pages().await.err().unwrap();
            assert_eq!(err.kind, ErrorKind::Config, "{keys:?}");
            assert!(err.message.contains(clash), "{keys:?}: {err}");
        }
    }

    #[test]
    fn opening_reports_failures() {
        assert!(open(&[], "x").unwrap_err().contains("opener"));
    }

    #[tokio::test]
    async fn opening_runs_the_command_with_the_target() {
        let dir = std::env::temp_dir().join(format!("tui-open-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let out = dir.join("opened");
        let opener = vec![
            "/bin/sh".to_owned(),
            "-c".to_owned(),
            format!("echo \"$0\" > '{}'", out.display()),
        ];
        open(&opener, "https://example.com/a").unwrap();
        for _ in 0..100 {
            if std::fs::read_to_string(&out).is_ok_and(|s| !s.is_empty()) {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert_eq!(
            std::fs::read_to_string(&out).unwrap().trim(),
            "https://example.com/a"
        );
        assert!(open(&["/no/such/opener".to_owned()], "x").is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn refuses_to_run_without_a_terminal() {
        // cargo test 的 stdout 不是终端
        let (_app, service) = service(vec![('d', 1)]).await;
        if !std::io::stdout().is_terminal() {
            let err = service.run().await.unwrap_err();
            assert!(err.message.contains("终端"), "{err}");
        }
    }

    #[test]
    fn keys() {
        let press = |code, modifiers| event::KeyEvent::new(code, modifiers);
        assert_eq!(
            key_of(&press(KeyCode::Char('c'), KeyModifiers::CONTROL)),
            Key::Interrupt
        );
        assert_eq!(
            key_of(&press(KeyCode::Char('c'), KeyModifiers::NONE)),
            Key::Char('c')
        );
        assert_eq!(
            key_of(&press(KeyCode::BackTab, KeyModifiers::SHIFT)),
            Key::BackTab
        );
        assert_eq!(
            key_of(&press(KeyCode::F(5), KeyModifiers::NONE)),
            Key::Refresh
        );
        assert_eq!(
            key_of(&press(KeyCode::F(6), KeyModifiers::NONE)),
            Key::Other
        );
        assert_eq!(
            key_of(&press(KeyCode::Char('J'), KeyModifiers::SHIFT)),
            Key::Char('J')
        );
    }

    #[test]
    fn config() {
        assert_eq!(
            ConfigSection::default().parse::<Config>().unwrap(),
            Config::default()
        );
    }
}
