//! 把状态画到 ratatui 的帧上。

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout};
use ratatui::style::{Color, Modifier, Style as RStyle};
use ratatui::text::{Line as RLine, Span as RSpan, Text};
use ratatui::widgets::Paragraph;
use tui_contract::{Line, Style};

use crate::app::{App, layout_selected};

fn style(style: Style) -> RStyle {
    match style {
        Style::Normal => RStyle::default(),
        Style::Dim => RStyle::default().fg(Color::DarkGray),
        Style::Accent => RStyle::default().fg(Color::Cyan),
        Style::Strong => RStyle::default().add_modifier(Modifier::BOLD),
    }
}

fn convert(line: &Line) -> RLine<'static> {
    RLine::from(
        line.spans
            .iter()
            .map(|s| RSpan::styled(s.text.clone(), style(s.style)))
            .collect::<Vec<_>>(),
    )
}

const HELP: &str = "  ←/→ 前后翻  . 回到现在  ↑/↓ 滚动  Tab 换页  r 刷新  q 退出";
const HELP_INPUT: &str = "  / 输入  ↑/↓ 滚动  Tab 换页  r 刷新  q 退出";
const HELP_EDITING: &str = "  Enter 确定  Esc 取消  Ctrl-C 退出";

/// 有页面按键时的帮助：先页面的键，再外壳的键。
fn help_with_actions(app: &App) -> Option<String> {
    let page = app.pages.get(app.current)?;
    if page.actions.is_empty() {
        return None;
    }
    let mut text = String::from(" ");
    for a in &page.actions {
        text.push_str(&format!(" {} {} ", a.key, a.label));
    }
    let refresh = if app.r_is_taken() { "F5" } else { "r" };
    text.push_str(&format!(" │ j/k 选择  Tab 换页  {refresh} 刷新  q 退出"));
    Some(text)
}

/// 详情区最多占主体的多少（百分比）。
const DETAIL_SHARE: u16 = 45;

pub(crate) fn draw(frame: &mut Frame<'_>, app: &App) {
    // 页面按键要的输入优先于页面自己的输入框
    let prompt = app
        .asking
        .as_ref()
        .map(|a| a.prompt.as_str())
        .or_else(|| app.prompt());
    let [tabs, input, body, help] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(u16::from(prompt.is_some())),
        Constraint::Min(1),
        Constraint::Length(1),
    ])
    .areas(frame.area());

    let mut spans = vec![RSpan::raw(" ")];
    for (i, page) in app.pages.iter().enumerate() {
        let label = format!(" {} {} ", page.key, page.title);
        let s = if i == app.current {
            RStyle::default().add_modifier(Modifier::REVERSED | Modifier::BOLD)
        } else {
            RStyle::default().fg(Color::DarkGray)
        };
        spans.push(RSpan::styled(label, s));
        spans.push(RSpan::raw(" "));
    }
    frame.render_widget(Paragraph::new(RLine::from(spans)), tabs);

    // 选中的行有详情时，主体下半部分是详情
    let detail: &[Line] = app.selected_row().map_or(&[], |r| r.detail.as_slice());
    let (body, detail_area) = if detail.is_empty() {
        (body, None)
    } else {
        let want = u16::try_from(detail.len() + 1).unwrap_or(u16::MAX);
        let room = body.height.saturating_mul(DETAIL_SHARE) / 100;
        let [top, bottom] =
            Layout::vertical([Constraint::Min(3), Constraint::Length(want.min(room))]).areas(body);
        (top, Some(bottom))
    };

    let mut selected_line = None;
    let lines: Vec<RLine<'static>> = match (&app.view, app.pages.is_empty()) {
        (_, true) => vec![RLine::styled("  没有组件登记页面", style(Style::Dim))],
        (None, false) => vec![RLine::styled("  读取中……", style(Style::Dim))],
        (Some(Err(why)), false) => vec![RLine::styled(
            format!("  出错了：{why}"),
            RStyle::default().fg(Color::Red),
        )],
        (Some(Ok(view)), false) => {
            let laid = layout_selected(view, body.width, app.selected_index());
            selected_line = laid.selected;
            laid.lines
                .iter()
                .enumerate()
                .map(|(i, l)| {
                    let line = convert(l);
                    if Some(i) == laid.selected {
                        line.style(RStyle::default().add_modifier(Modifier::REVERSED))
                    } else {
                        line
                    }
                })
                .collect()
        }
    };
    // 主体上下各留一行空白；滚动上限按真正看得见的高度算，否则滚到底时最后几行会被截掉
    let visible = body.inner(ratatui::layout::Margin {
        horizontal: 0,
        vertical: 1,
    });
    let max_scroll = u16::try_from(lines.len())
        .unwrap_or(u16::MAX)
        .saturating_sub(visible.height);
    // 有选中的行时，滚到让它看得见（它在最底下一行为止）
    let scroll = match selected_line {
        Some(line) => u16::try_from(line)
            .unwrap_or(u16::MAX)
            .saturating_sub(visible.height.saturating_sub(1)),
        None => app.scroll,
    };
    let body_text = Paragraph::new(Text::from(lines)).scroll((scroll.min(max_scroll), 0));
    frame.render_widget(body_text, visible);

    if let Some(area) = detail_area {
        let mut lines = vec![RLine::styled(
            "─".repeat(usize::from(area.width)),
            style(Style::Dim),
        )];
        lines.extend(detail.iter().map(convert));
        frame.render_widget(Paragraph::new(Text::from(lines)), area);
    }

    if let Some(prompt) = prompt {
        let mut spans = vec![RSpan::styled(format!("  {prompt}："), style(Style::Dim))];
        // 页面按键要的输入还没开始编辑时不会出现，所以有 asking 就一定在编辑
        match &app.editing {
            Some(buffer) => {
                spans.push(RSpan::styled(buffer.clone(), style(Style::Strong)));
                spans.push(RSpan::styled("▏", style(Style::Accent)));
            }
            None => spans.push(RSpan::raw(app.input().to_owned())),
        }
        frame.render_widget(Paragraph::new(RLine::from(spans)), input);
    }

    let help_line = match (&app.message, app.editing.is_some()) {
        (Some(message), false) => RLine::styled(format!("  {message}"), style(Style::Accent)),
        (_, true) => RLine::styled(HELP_EDITING, style(Style::Dim)),
        (None, false) => match help_with_actions(app) {
            Some(text) => RLine::styled(text, style(Style::Dim)),
            None if prompt.is_some() => RLine::styled(HELP_INPUT, style(Style::Dim)),
            None => RLine::styled(HELP, style(Style::Dim)),
        },
    };
    frame.render_widget(Paragraph::new(help_line), help);
}

#[cfg(test)]
mod tests {
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use tui_contract::{Bar, Block, PageSpec, PageView};

    use super::*;

    fn screen(app: &App, w: u16, h: u16) -> Vec<String> {
        let mut terminal = Terminal::new(TestBackend::new(w, h)).unwrap();
        terminal.draw(|f| draw(f, app)).unwrap();
        let buffer = terminal.backend().buffer().clone();
        (0..h)
            .map(|y| {
                // 中文占两格，第二格在缓冲区里是个占位的空格，跳过
                let mut row = String::new();
                let mut x = 0;
                while x < w {
                    let symbol = buffer[(x, y)].symbol();
                    row.push_str(symbol);
                    x += u16::try_from(unicode_width::UnicodeWidthStr::width(symbol).max(1))
                        .unwrap_or(1);
                }
                row.trim_end().to_owned()
            })
            .collect()
    }

    fn app() -> App {
        let mut app = App::new(vec![
            PageSpec {
                id: "day".to_owned(),
                title: "日".to_owned(),
                key: 'd',
                order: 1,
                input: None,
                actions: vec![],
            },
            PageSpec {
                id: "week".to_owned(),
                title: "周".to_owned(),
                key: 'w',
                order: 2,
                input: None,
                actions: vec![],
            },
        ]);
        app.view = Some(Ok(PageView {
            heading: "2026-10-20 周二".to_owned(),
            note: "共 40 分".to_owned(),
            blocks: vec![Block::Bars {
                bars: vec![Bar {
                    label: "zen".to_owned(),
                    value: 1,
                    value_text: "40 分".to_owned(),
                }],
            }],
        }));
        app
    }

    #[test]
    fn draws_tabs_page_and_help() {
        let rows = screen(&app(), 70, 8);
        assert!(
            rows[0].contains("d 日") && rows[0].contains("w 周"),
            "{rows:#?}"
        );
        assert_eq!(rows[1], "");
        assert!(
            rows[2].contains("2026-10-20 周二") && rows[2].contains("共 40 分"),
            "{rows:#?}"
        );
        assert!(
            rows[4].contains("40 分") && rows[4].contains('█') && rows[4].ends_with("zen"),
            "{rows:#?}"
        );
        assert!(rows[7].contains("q 退出"), "{rows:#?}");
    }

    #[test]
    fn the_input_box_and_its_help() {
        let mut a = app();
        a.pages[0].input = Some("找什么".to_owned());
        a.editing = Some("气象".to_owned());
        let rows = screen(&a, 70, 8);
        assert_eq!(rows[1], "  找什么：气象▏", "{rows:#?}");
        assert!(rows[7].contains("Enter 确定"), "{rows:#?}");
        a.inputs[0] = "气象站".to_owned();
        a.editing = None;
        let rows = screen(&a, 70, 8);
        assert_eq!(rows[1], "  找什么：气象站");
        assert!(rows[7].contains("/ 输入"), "{rows:#?}");
    }

    #[test]
    fn shows_errors_and_empty_states() {
        let mut a = app();
        a.view = Some(Err("数据库打不开".to_owned()));
        assert!(
            screen(&a, 60, 6)
                .iter()
                .any(|r| r.contains("出错了：数据库打不开"))
        );
        a.view = None;
        assert!(screen(&a, 60, 6).iter().any(|r| r.contains("读取中")));
        let empty = App::new(vec![]);
        assert!(
            screen(&empty, 60, 6)
                .iter()
                .any(|r| r.contains("没有组件登记页面"))
        );
    }

    #[test]
    fn scrolling_moves_the_body_but_not_beyond_the_end() {
        let mut a = app();
        a.view = Some(Ok(PageView {
            heading: "h".to_owned(),
            note: String::new(),
            blocks: vec![Block::Lines {
                lines: (0..20)
                    .map(|i| tui_contract::Line {
                        spans: vec![tui_contract::Span::new(
                            format!("  第 {i} 行"),
                            Style::Normal,
                        )],
                    })
                    .collect(),
            }],
        }));
        a.scroll = 5;
        let rows = screen(&a, 40, 10);
        assert!(rows[2].contains("第 3 行"), "{rows:#?}");
        a.scroll = 1000;
        let rows = screen(&a, 40, 10);
        assert!(rows[7].contains("第 19 行"), "滚到底就停：{rows:#?}");
    }

    #[test]
    fn a_list_with_the_selected_row_its_detail_and_page_keys() {
        use tui_contract::{ActionSpec, ListRow, Span};
        let row = |id: &str, text: &str| ListRow {
            id: id.to_owned(),
            line: Line {
                spans: vec![Span::new(text, Style::Normal)],
            },
            detail: vec![
                Line {
                    spans: vec![Span::new(format!("  {text} 的详情"), Style::Normal)],
                },
                Line {
                    spans: vec![Span::new("  来源  zen", Style::Dim)],
                },
            ],
        };
        let mut a = app();
        a.pages[0].actions = vec![
            ActionSpec {
                key: 'x',
                label: "完成".to_owned(),
                confirm: false,
            },
            ActionSpec {
                key: 'r',
                label: "稍后读".to_owned(),
                confirm: false,
            },
        ];
        a.loaded(Ok(PageView {
            heading: "收件箱".to_owned(),
            note: "2 条".to_owned(),
            blocks: vec![Block::List {
                rows: vec![row("a", "回复老王"), row("b", "看 PR")],
            }],
        }));
        a.handle(crate::app::Key::Char('j'));
        let rows = screen(&a, 70, 14);
        assert!(rows.iter().any(|r| r == "  回复老王"), "{rows:#?}");
        assert!(rows.iter().any(|r| r == "› 看 PR"), "{rows:#?}");
        let rule = rows
            .iter()
            .position(|r| r.starts_with('─'))
            .expect("详情区的分隔线");
        assert_eq!(rows[rule + 1], "  看 PR 的详情", "{rows:#?}");
        assert!(
            rows[13].contains("x 完成") && rows[13].contains("F5 刷新"),
            "{rows:#?}"
        );

        // 有提示时帮助行换成提示
        a.message = Some("已完成".to_owned());
        assert_eq!(screen(&a, 70, 14)[13], "  已完成");

        // 页面要输入时，输入框用页面给的提示
        a.message = None;
        a.asking = Some(crate::app::Asking {
            key: 'e',
            row: None,
            prompt: "改成".to_owned(),
        });
        a.editing = Some("新文字".to_owned());
        assert_eq!(screen(&a, 70, 14)[1], "  改成：新文字▏");
    }

    #[test]
    fn a_long_list_scrolls_to_keep_the_selection_visible() {
        use tui_contract::{ListRow, Span};
        let mut a = app();
        a.loaded(Ok(PageView {
            heading: "h".to_owned(),
            note: String::new(),
            blocks: vec![Block::List {
                rows: (0..30)
                    .map(|i| ListRow {
                        id: i.to_string(),
                        line: Line {
                            spans: vec![Span::new(format!("第 {i} 条"), Style::Normal)],
                        },
                        detail: vec![],
                    })
                    .collect(),
            }],
        }));
        a.selected[0] = 25;
        let rows = screen(&a, 40, 10);
        assert!(rows.iter().any(|r| r.contains("› 第 25 条")), "{rows:#?}");
    }
}
