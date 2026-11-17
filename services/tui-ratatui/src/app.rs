//! 界面的状态和按键处理，以及把页面内容排成行。纯逻辑，不碰终端。

use tui_contract::{
    ActionRequest, ActionResult, Bar, Block, Line, ListRow, PageSpec, PageView, Span, Style,
};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

/// 外壳关心的按键。和终端库无关。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Key {
    Char(char),
    Left,
    Right,
    Up,
    Down,
    PageUp,
    PageDown,
    Home,
    Tab,
    BackTab,
    Esc,
    Enter,
    Backspace,
    /// Ctrl-C
    Interrupt,
    /// F5：刷新（页面把 `r` 占作自己的按键时也能刷新）。
    Refresh,
    Other,
}

/// 外壳自己用的字符键，页面的按键不能用。
pub(crate) const RESERVED: [char; 7] = ['q', '/', 'j', 'k', 'h', 'l', '.'];

/// 按键之后要做什么。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Effect {
    Nothing,
    /// 只要重画。
    Redraw,
    /// 要重新取当前页的内容。
    Reload,
    /// 把页面的按键交给页面。
    Act(ActionRequest),
    Quit,
}

/// 页面要过一段输入，正在输入中。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Asking {
    pub(crate) key: char,
    pub(crate) row: Option<String>,
    pub(crate) prompt: String,
}

/// 界面的状态。
pub(crate) struct App {
    pub(crate) pages: Vec<PageSpec>,
    pub(crate) current: usize,
    pub(crate) offset: i64,
    pub(crate) scroll: u16,
    /// 当前页的内容；取失败时是给人看的原因。
    pub(crate) view: Option<Result<PageView, String>>,
    /// 每个页面输入框里提交过的文字（没有输入框的页面一直是空的）。
    pub(crate) inputs: Vec<String>,
    /// 正在输入时，输入框里的文字。这时所有按键都进输入框。
    pub(crate) editing: Option<String>,
    /// 正在输入的是页面按键要的输入（而不是页面自己的输入框）。
    pub(crate) asking: Option<Asking>,
    /// 每个页面列表里选中的是第几行。
    pub(crate) selected: Vec<usize>,
    /// 重新取内容之后要选中的行。
    pub(crate) select_after_reload: Option<String>,
    /// 按了一次、等着再按一次确认的键。
    pub(crate) confirming: Option<char>,
    /// 显示在帮助行位置的一句话，按下一个键就消失。
    pub(crate) message: Option<String>,
}

impl App {
    pub(crate) fn new(pages: Vec<PageSpec>) -> Self {
        let mut app = Self {
            inputs: vec![String::new(); pages.len()],
            selected: vec![0; pages.len()],
            pages,
            current: 0,
            offset: 0,
            scroll: 0,
            view: None,
            editing: None,
            asking: None,
            select_after_reload: None,
            confirming: None,
            message: None,
        };
        app.start_editing_if_empty();
        app
    }

    /// 当前页面的输入框提示语；没有输入框是 `None`。
    pub(crate) fn prompt(&self) -> Option<&str> {
        self.pages.get(self.current)?.input.as_deref()
    }

    /// 当前页面里可以选中的列表（第一个 `List` 块）。
    pub(crate) fn rows(&self) -> Option<&[ListRow]> {
        match &self.view {
            Some(Ok(view)) => view.blocks.iter().find_map(|b| match b {
                Block::List { rows } => Some(rows.as_slice()),
                _ => None,
            }),
            _ => None,
        }
    }

    /// 选中的是第几行（已经按列表长度收拢）。没有列表或者列表是空的是 `None`。
    pub(crate) fn selected_index(&self) -> Option<usize> {
        let rows = self.rows()?;
        let i = *self.selected.get(self.current)?;
        (!rows.is_empty()).then(|| i.min(rows.len() - 1))
    }

    pub(crate) fn selected_row(&self) -> Option<&ListRow> {
        self.rows()?.get(self.selected_index()?)
    }

    /// 新内容到了：按要求选中某一行，或者把原来的位置收拢到列表里。
    pub(crate) fn loaded(&mut self, view: Result<PageView, String>) {
        self.view = Some(view);
        let wanted = self.select_after_reload.take();
        let found = wanted.and_then(|id| self.rows()?.iter().position(|r| r.id == id));
        let len = self.rows().map_or(0, <[ListRow]>::len);
        if let Some(slot) = self.selected.get_mut(self.current) {
            *slot = found.unwrap_or((*slot).min(len.saturating_sub(1)));
        }
    }

    /// 页面处理完按键的结果。返回要不要重取内容，以及要打开的东西。
    pub(crate) fn apply(
        &mut self,
        key: char,
        row: Option<String>,
        result: ActionResult,
    ) -> (Effect, Option<String>) {
        self.message = result.message;
        if let Some(ask) = result.ask {
            self.editing = Some(ask.initial);
            self.asking = Some(Asking {
                key,
                row,
                prompt: ask.prompt,
            });
        }
        self.select_after_reload = result.select;
        let effect = if result.reload {
            Effect::Reload
        } else {
            Effect::Redraw
        };
        (effect, result.open)
    }

    /// 当前页面里的某个按键。
    fn action(&self, c: char) -> Option<&tui_contract::ActionSpec> {
        self.pages
            .get(self.current)?
            .actions
            .iter()
            .find(|a| a.key == c)
    }

    /// 页面占用了 `r`。
    pub(crate) fn r_is_taken(&self) -> bool {
        self.action('r').is_some()
    }

    fn move_selection(&mut self, delta: i64) -> Effect {
        let Some(rows) = self.rows() else {
            return Effect::Nothing;
        };
        if rows.is_empty() {
            return Effect::Nothing;
        }
        let last = i64::try_from(rows.len() - 1).unwrap_or(i64::MAX);
        let now = i64::try_from(self.selected_index().unwrap_or(0)).unwrap_or(0);
        let next = (now + delta).clamp(0, last);
        if next == now {
            return Effect::Nothing;
        }
        if let Some(slot) = self.selected.get_mut(self.current) {
            *slot = usize::try_from(next).unwrap_or(0);
        }
        Effect::Redraw
    }

    /// 当前页面提交过的文字。
    pub(crate) fn input(&self) -> &str {
        self.inputs.get(self.current).map_or("", String::as_str)
    }

    /// 切到一个带输入框、还什么都没输入过的页面时，直接开始输入。
    fn start_editing_if_empty(&mut self) {
        if self.prompt().is_some() && self.input().is_empty() {
            self.editing = Some(String::new());
        }
    }

    fn switch(&mut self, index: usize) -> Effect {
        if index == self.current {
            return Effect::Nothing;
        }
        // 不同页面的「一格」意思不同（一天、一周），切页时回到现在
        self.current = index;
        self.offset = 0;
        self.scroll = 0;
        self.editing = None;
        self.asking = None;
        self.start_editing_if_empty();
        Effect::Reload
    }

    /// 输入中的按键：字都进输入框，Enter 提交，Esc 放弃。
    fn edit(&mut self, key: Key) -> Effect {
        let Some(buffer) = self.editing.as_mut() else {
            return Effect::Nothing;
        };
        match key {
            Key::Char(c) => {
                buffer.push(c);
                Effect::Redraw
            }
            Key::Backspace => {
                buffer.pop();
                Effect::Redraw
            }
            Key::Enter => {
                let text = buffer.trim().to_owned();
                self.editing = None;
                if let Some(asking) = self.asking.take() {
                    return Effect::Act(ActionRequest {
                        key: asking.key,
                        row: asking.row,
                        input: Some(text),
                    });
                }
                if let Some(slot) = self.inputs.get_mut(self.current) {
                    *slot = text;
                }
                self.offset = 0;
                self.scroll = 0;
                Effect::Reload
            }
            Key::Esc => {
                self.editing = None;
                self.asking = None;
                Effect::Redraw
            }
            Key::Interrupt => Effect::Quit,
            _ => Effect::Nothing,
        }
    }

    pub(crate) fn handle(&mut self, key: Key) -> Effect {
        if self.editing.is_some() {
            return self.edit(key);
        }
        let had_message = self.message.take().is_some();
        let confirming = self.confirming.take();
        if key == Key::Esc && confirming.is_some() {
            return Effect::Redraw;
        }
        let count = self.pages.len();
        if let Key::Char(c) = key
            && !self.pages.iter().any(|p| p.key == c)
            && let Some(action) = self.action(c)
        {
            if action.confirm && confirming != Some(c) {
                self.message = Some(format!("再按一次 {c} 确认「{}」", action.label));
                self.confirming = Some(c);
                return Effect::Redraw;
            }
            return Effect::Act(ActionRequest {
                key: c,
                row: self.selected_row().map(|r| r.id.clone()),
                input: None,
            });
        }
        let effect = self.navigate(key, count);
        if effect == Effect::Nothing && (had_message || confirming.is_some()) {
            Effect::Redraw
        } else {
            effect
        }
    }

    fn navigate(&mut self, key: Key, count: usize) -> Effect {
        let has_list = self.rows().is_some();
        match key {
            Key::Char('/') if self.prompt().is_some() => {
                self.editing = Some(self.input().to_owned());
                Effect::Redraw
            }
            Key::Char('q') | Key::Esc | Key::Interrupt => Effect::Quit,
            _ if count == 0 => Effect::Nothing,
            Key::Left | Key::Char('h') => {
                self.offset -= 1;
                self.scroll = 0;
                Effect::Reload
            }
            Key::Right | Key::Char('l') => {
                if self.offset >= 0 {
                    return Effect::Nothing;
                }
                self.offset += 1;
                self.scroll = 0;
                Effect::Reload
            }
            Key::Home | Key::Char('.') => {
                self.offset = 0;
                self.scroll = 0;
                Effect::Reload
            }
            Key::Char('r') | Key::Refresh => Effect::Reload,
            Key::Tab => self.switch((self.current + 1) % count),
            Key::BackTab => self.switch((self.current + count - 1) % count),
            Key::Up | Key::Char('k') if has_list => self.move_selection(-1),
            Key::Down | Key::Char('j') if has_list => self.move_selection(1),
            Key::PageUp if has_list => self.move_selection(-10),
            Key::PageDown if has_list => self.move_selection(10),
            Key::Up | Key::Char('k') => self.scroll_by(-1),
            Key::Down | Key::Char('j') => self.scroll_by(1),
            Key::PageUp => self.scroll_by(-10),
            Key::PageDown => self.scroll_by(10),
            Key::Char(c) => match self.pages.iter().position(|p| p.key == c) {
                Some(index) => self.switch(index),
                None => Effect::Nothing,
            },
            Key::Enter | Key::Backspace | Key::Other => Effect::Nothing,
        }
    }

    fn scroll_by(&mut self, delta: i32) -> Effect {
        let next = (i32::from(self.scroll) + delta).clamp(0, i32::from(u16::MAX));
        let next = u16::try_from(next).unwrap_or(0);
        if next == self.scroll {
            return Effect::Nothing;
        }
        self.scroll = next;
        Effect::Redraw
    }
}

fn width(text: &str) -> usize {
    text.width()
}

/// 截到不超过 `max` 列，截掉了就以 `…` 结尾。
pub(crate) fn truncate(text: &str, max: usize) -> String {
    if width(text) <= max {
        return text.to_owned();
    }
    let mut out = String::new();
    let mut used = 0;
    for c in text.chars() {
        let w = c.width().unwrap_or(0);
        if used + w + 1 > max {
            break;
        }
        out.push(c);
        used += w;
    }
    out.push('…');
    out
}

fn pad_left(text: &str, to: usize) -> String {
    format!("{}{text}", " ".repeat(to.saturating_sub(width(text))))
}

fn pad_right(text: &str, to: usize) -> String {
    format!("{text}{}", " ".repeat(to.saturating_sub(width(text))))
}

fn line(spans: Vec<Span>) -> Line {
    Line { spans }
}

fn bars(bars: &[Bar], total_width: usize) -> Vec<Line> {
    let value_width = bars.iter().map(|b| width(&b.value_text)).max().unwrap_or(0);
    let max = bars.iter().map(|b| b.value).max().unwrap_or(0).max(1);
    // 两边各留两格；数值、条、名字之间各两格
    let room = total_width.saturating_sub(2 + value_width + 2 + 2);
    let bar_width = (room / 3).clamp(4, 30);
    let label_width = room.saturating_sub(bar_width).max(8);
    bars.iter()
        .map(|b| {
            let filled = usize::try_from(
                (b.value.max(0) * i64::try_from(bar_width).unwrap_or(0) + max / 2) / max,
            )
            .unwrap_or(0)
            .clamp(usize::from(b.value > 0), bar_width);
            line(vec![
                Span::new(
                    format!("  {}  ", pad_left(&b.value_text, value_width)),
                    Style::Normal,
                ),
                Span::new("█".repeat(filled), Style::Accent),
                Span::new(" ".repeat(bar_width - filled + 2), Style::Normal),
                Span::new(truncate(&b.label, label_width), Style::Normal),
            ])
        })
        .collect()
}

fn table(header: &[String], rows: &[Vec<String>], total_width: usize) -> Vec<Line> {
    let columns = header
        .len()
        .max(rows.iter().map(Vec::len).max().unwrap_or(0));
    let mut widths = vec![0; columns];
    for row in std::iter::once(header).chain(rows.iter().map(Vec::as_slice)) {
        for (i, cell) in row.iter().enumerate() {
            widths[i] = widths[i].max(width(cell));
        }
    }
    // 第一列（名字）挤一挤，让数字列放得下
    let others: usize = widths.iter().skip(1).map(|w| w + 2).sum();
    if let Some(first) = widths.first_mut() {
        *first = (*first).min(total_width.saturating_sub(2 + others).max(6));
    }
    let render = |row: &[String], style: Style| {
        let mut text = String::from("  ");
        for (i, w) in widths.iter().enumerate() {
            let cell = row.get(i).map_or("", String::as_str);
            if i == 0 {
                text.push_str(&pad_right(&truncate(cell, *w), *w));
            } else {
                text.push_str("  ");
                text.push_str(&pad_left(cell, *w));
            }
        }
        line(vec![Span::new(text, style)])
    };
    std::iter::once(render(header, Style::Dim))
        .chain(rows.iter().map(|r| render(r, Style::Normal)))
        .collect()
}

/// 排好的一页。
pub(crate) struct Laid {
    pub(crate) lines: Vec<Line>,
    /// 选中的那一行在 `lines` 里的位置。
    pub(crate) selected: Option<usize>,
}

/// 把一页内容排成行。`width` 是可用的列数。
#[cfg(test)]
pub(crate) fn layout(view: &PageView, total_width: u16) -> Vec<Line> {
    layout_selected(view, total_width, None).lines
}

/// 同 [`layout`]，并标出列表里选中的行：行首画 `›`，返回它的位置。
pub(crate) fn layout_selected(view: &PageView, total_width: u16, selected: Option<usize>) -> Laid {
    let total_width = usize::from(total_width);
    let mut first_list = true;
    let mut selected_line = None;
    let mut out = vec![line(vec![
        Span::new(format!("  {}", view.heading), Style::Strong),
        Span::new(format!("  {}", view.note), Style::Dim),
    ])];
    for block in &view.blocks {
        out.push(Line::default());
        match block {
            Block::Heading { text } => {
                out.push(line(vec![Span::new(format!("  {text}"), Style::Strong)]))
            }
            Block::Lines { lines } => out.extend(lines.iter().cloned()),
            Block::Bars { bars: items } => out.extend(bars(items, total_width)),
            Block::Table { header, rows } => out.extend(table(header, rows, total_width)),
            Block::Empty { message } => {
                out.push(line(vec![Span::new(format!("  {message}"), Style::Dim)]))
            }
            Block::List { rows } => {
                let mark = if first_list { selected } else { None };
                first_list = false;
                for (i, row) in rows.iter().enumerate() {
                    let here = mark == Some(i);
                    if here {
                        selected_line = Some(out.len());
                    }
                    let mut spans = vec![Span::new(if here { "› " } else { "  " }, Style::Accent)];
                    spans.extend(row.line.spans.iter().cloned());
                    out.push(line(spans));
                }
            }
        }
    }
    Laid {
        lines: out,
        selected: selected_line,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec(id: &str, key: char) -> PageSpec {
        PageSpec {
            id: id.to_owned(),
            title: id.to_owned(),
            key,
            order: 0,
            input: None,
            actions: vec![],
        }
    }

    fn app() -> App {
        App::new(vec![
            spec("day", 'd'),
            spec("timeline", 't'),
            spec("week", 'w'),
        ])
    }

    fn text(lines: &[Line]) -> Vec<String> {
        lines
            .iter()
            .map(|l| {
                l.spans
                    .iter()
                    .map(|s| s.text.as_str())
                    .collect::<String>()
                    .trim_end()
                    .to_owned()
            })
            .collect()
    }

    fn with_search() -> App {
        let mut search = spec("search", 's');
        search.input = Some("找什么".to_owned());
        App::new(vec![spec("day", 'd'), search])
    }

    #[test]
    fn an_input_page_starts_editing_the_first_time() {
        let mut a = with_search();
        assert_eq!(a.editing, None, "第一页没有输入框");
        assert_eq!(a.handle(Key::Char('s')), Effect::Reload);
        assert_eq!(a.editing.as_deref(), Some(""));
        assert_eq!(a.prompt(), Some("找什么"));
    }

    #[test]
    fn while_editing_every_key_goes_into_the_box() {
        let mut a = with_search();
        a.handle(Key::Char('s'));
        for c in "q气象d".chars() {
            assert_eq!(
                a.handle(Key::Char(c)),
                Effect::Redraw,
                "{c} 不是退出也不是换页"
            );
        }
        assert_eq!(a.handle(Key::Backspace), Effect::Redraw);
        assert_eq!(a.current, 1);
        assert_eq!(a.handle(Key::Enter), Effect::Reload);
        assert_eq!((a.editing.as_deref(), a.input()), (None, "q气象"));

        // 改：带着上次的文字进去，Esc 放弃改动
        assert_eq!(a.handle(Key::Char('/')), Effect::Redraw);
        assert_eq!(a.editing.as_deref(), Some("q气象"));
        a.handle(Key::Char('x'));
        assert_eq!(a.handle(Key::Esc), Effect::Redraw);
        assert_eq!((a.editing.as_deref(), a.input()), (None, "q气象"));

        // 切走再回来：提交过就不自动进入输入
        a.handle(Key::Char('d'));
        a.handle(Key::Char('s'));
        assert_eq!(a.editing, None);
        assert_eq!(a.input(), "q气象");
        assert_eq!(
            a.handle(Key::Char('q')),
            Effect::Quit,
            "不在输入时 q 照样退出"
        );
    }

    #[test]
    fn slash_does_nothing_without_an_input_box() {
        let mut a = app();
        assert_eq!(a.handle(Key::Char('/')), Effect::Nothing);
        assert_eq!(a.editing, None);
    }

    #[test]
    fn ctrl_c_quits_even_while_editing() {
        let mut a = with_search();
        a.handle(Key::Char('s'));
        assert_eq!(a.handle(Key::Interrupt), Effect::Quit);
    }

    #[test]
    fn paging_back_and_forth_never_goes_past_now() {
        let mut a = app();
        assert_eq!(a.handle(Key::Right), Effect::Nothing, "已经是现在了");
        assert_eq!(a.handle(Key::Left), Effect::Reload);
        assert_eq!(a.handle(Key::Char('h')), Effect::Reload);
        assert_eq!(a.offset, -2);
        assert_eq!(a.handle(Key::Right), Effect::Reload);
        assert_eq!(a.offset, -1);
        assert_eq!(a.handle(Key::Home), Effect::Reload);
        assert_eq!(a.offset, 0);
    }

    #[test]
    fn switching_pages_resets_offset_and_scroll() {
        let mut a = app();
        a.handle(Key::Left);
        a.handle(Key::Down);
        assert_eq!(a.handle(Key::Char('w')), Effect::Reload);
        assert_eq!((a.current, a.offset, a.scroll), (2, 0, 0));
        assert_eq!(a.handle(Key::Char('w')), Effect::Nothing, "已经在这一页");
        assert_eq!(a.handle(Key::Tab), Effect::Reload);
        assert_eq!(a.current, 0, "循环");
        assert_eq!(a.handle(Key::BackTab), Effect::Reload);
        assert_eq!(a.current, 2);
        assert_eq!(a.handle(Key::Char('z')), Effect::Nothing);
    }

    #[test]
    fn scrolling_and_quitting() {
        let mut a = app();
        assert_eq!(a.handle(Key::Up), Effect::Nothing, "已经在顶上");
        assert_eq!(a.handle(Key::PageDown), Effect::Redraw);
        assert_eq!(a.scroll, 10);
        assert_eq!(a.handle(Key::Char('k')), Effect::Redraw);
        assert_eq!(a.scroll, 9);
        for key in [Key::Char('q'), Key::Esc, Key::Interrupt] {
            assert_eq!(app().handle(key), Effect::Quit);
        }
    }

    #[test]
    fn with_no_pages_only_quitting_works() {
        let mut a = App::new(vec![]);
        assert_eq!(a.handle(Key::Left), Effect::Nothing);
        assert_eq!(a.handle(Key::Tab), Effect::Nothing);
        assert_eq!(a.handle(Key::Char('q')), Effect::Quit);
    }

    #[test]
    fn bars_are_scaled_and_labels_cut_to_width() {
        let view = PageView {
            heading: "2026-10-20 周二".to_owned(),
            note: "共 2 小时".to_owned(),
            blocks: vec![Block::Bars {
                bars: vec![
                    Bar {
                        label: "ghostty".to_owned(),
                        value: 100,
                        value_text: "1 小时 40 分".to_owned(),
                    },
                    Bar {
                        label: "一个非常长的名字".repeat(5),
                        value: 25,
                        value_text: "25 分".to_owned(),
                    },
                ],
            }],
        };
        let lines = text(&layout(&view, 60));
        assert_eq!(lines[0], "  2026-10-20 周二  共 2 小时");
        assert_eq!(lines[1], "");
        assert!(
            lines[2].starts_with("  1 小时 40 分  ████████████"),
            "{:?}",
            lines[2]
        );
        assert!(lines[2].ends_with("ghostty"));
        assert!(
            lines[3].starts_with("         25 分  ████  "),
            "{:?}",
            lines[3]
        );
        assert!(lines[3].ends_with('…'));
        for l in &lines {
            assert!(width(l) <= 60, "{l:?} 占了 {} 列", width(l));
        }
    }

    #[test]
    fn tables_align_numbers_right() {
        let view = PageView {
            heading: "周".to_owned(),
            note: String::new(),
            blocks: vec![Block::Table {
                header: vec![String::new(), "一".to_owned(), "合计".to_owned()],
                rows: vec![
                    vec!["ghostty".to_owned(), "1:45".to_owned(), "12:30".to_owned()],
                    vec!["zen".to_owned(), "·".to_owned(), "0:40".to_owned()],
                ],
            }],
        };
        let lines = text(&layout(&view, 80));
        assert_eq!(
            &lines[2..],
            [
                "             一   合计",
                "  ghostty  1:45  12:30",
                "  zen         ·   0:40"
            ]
        );
    }

    #[test]
    fn empty_and_heading_blocks() {
        let view = PageView {
            heading: "h".to_owned(),
            note: "n".to_owned(),
            blocks: vec![
                Block::Heading {
                    text: "按标题".to_owned(),
                },
                Block::Empty {
                    message: "没有记录".to_owned(),
                },
            ],
        };
        assert_eq!(
            text(&layout(&view, 40)),
            ["  h  n", "", "  按标题", "", "  没有记录"]
        );
    }

    // ---------- v2：列表、页面按键 ----------

    use tui_contract::{ActionSpec, Ask, ListRow};

    fn row(id: &str) -> ListRow {
        ListRow {
            id: id.to_owned(),
            line: line(vec![Span::new(format!("条目 {id}"), Style::Normal)]),
            detail: vec![line(vec![Span::new(format!("详情 {id}"), Style::Normal)])],
        }
    }

    fn action(key: char, label: &str, confirm: bool) -> ActionSpec {
        ActionSpec {
            key,
            label: label.to_owned(),
            confirm,
        }
    }

    /// 两页：「收件箱」有列表和按键，「周」没有。
    fn list_app(ids: &[&str]) -> App {
        let mut inbox = spec("inbox", '1');
        inbox.actions = vec![
            action('x', "完成", false),
            action('d', "删除", true),
            action('e', "改文字", false),
            action('r', "稍后读", false),
        ];
        let mut a = App::new(vec![inbox, spec("week", 'w')]);
        a.loaded(Ok(PageView {
            heading: "收件箱".to_owned(),
            note: String::new(),
            blocks: vec![Block::List {
                rows: ids.iter().map(|id| row(id)).collect(),
            }],
        }));
        a
    }

    fn act(key: char, row: Option<&str>, input: Option<&str>) -> Effect {
        Effect::Act(ActionRequest {
            key,
            row: row.map(str::to_owned),
            input: input.map(str::to_owned),
        })
    }

    #[test]
    fn j_and_k_move_the_selection_in_a_list() {
        let mut a = list_app(&["a", "b", "c"]);
        assert_eq!(a.selected_row().unwrap().id, "a");
        assert_eq!(a.handle(Key::Char('k')), Effect::Nothing, "已经在第一行");
        assert_eq!(a.handle(Key::Char('j')), Effect::Redraw);
        assert_eq!(a.handle(Key::Down), Effect::Redraw);
        assert_eq!(a.handle(Key::Char('j')), Effect::Nothing, "已经在最后一行");
        assert_eq!(a.selected_row().unwrap().id, "c");
        assert_eq!(a.handle(Key::PageUp), Effect::Redraw);
        assert_eq!(a.selected_row().unwrap().id, "a");
        assert_eq!(a.scroll, 0, "有列表时不滚动");
    }

    #[test]
    fn page_keys_go_to_the_page_with_the_selected_row() {
        let mut a = list_app(&["a", "b"]);
        a.handle(Key::Char('j'));
        assert_eq!(a.handle(Key::Char('x')), act('x', Some("b"), None));
        // 页面占了 r：r 是页面的键，刷新改用 F5
        assert_eq!(a.handle(Key::Char('r')), act('r', Some("b"), None));
        assert!(a.r_is_taken());
        assert_eq!(a.handle(Key::Refresh), Effect::Reload);
        // 别的页面没有这些键
        a.handle(Key::Char('w'));
        assert_eq!(a.handle(Key::Char('x')), Effect::Nothing);
        assert_eq!(a.handle(Key::Char('r')), Effect::Reload);
    }

    #[test]
    fn an_empty_list_still_gets_the_key_without_a_row() {
        let mut a = list_app(&[]);
        assert_eq!(a.selected_row(), None);
        assert_eq!(a.handle(Key::Char('x')), act('x', None, None));
    }

    #[test]
    fn confirm_keys_need_a_second_press() {
        let mut a = list_app(&["a"]);
        assert_eq!(a.handle(Key::Char('d')), Effect::Redraw);
        assert_eq!(a.message.as_deref(), Some("再按一次 d 确认「删除」"));
        assert_eq!(a.handle(Key::Char('d')), act('d', Some("a"), None));

        // 中间按了别的键就作废
        a.handle(Key::Char('d'));
        assert_eq!(a.handle(Key::Char('j')), Effect::Redraw, "清掉提示也要重画");
        assert_eq!(a.message, None);
        assert_eq!(a.handle(Key::Char('d')), Effect::Redraw, "又要重新确认");
        // Esc 只取消确认，不退出
        assert_eq!(a.handle(Key::Esc), Effect::Redraw);
        assert_eq!(a.handle(Key::Esc), Effect::Quit);
    }

    #[test]
    fn a_page_can_ask_for_input_and_gets_it_back_with_the_same_key_and_row() {
        let mut a = list_app(&["a", "b"]);
        a.handle(Key::Char('j'));
        let (effect, open) = a.apply(
            'e',
            Some("b".to_owned()),
            ActionResult {
                ask: Some(Ask {
                    prompt: "改成".to_owned(),
                    initial: "旧文字".to_owned(),
                }),
                ..ActionResult::default()
            },
        );
        assert_eq!((effect, open), (Effect::Redraw, None));
        assert_eq!(a.editing.as_deref(), Some("旧文字"));
        a.handle(Key::Backspace);
        a.handle(Key::Char('改'));
        assert_eq!(a.handle(Key::Enter), act('e', Some("b"), Some("旧文改")));
        assert_eq!((a.editing.as_ref(), a.asking.as_ref()), (None, None));

        // Esc 放弃：不调页面
        a.apply(
            'e',
            Some("b".to_owned()),
            ActionResult {
                ask: Some(Ask {
                    prompt: "改成".to_owned(),
                    initial: String::new(),
                }),
                ..ActionResult::default()
            },
        );
        assert_eq!(a.handle(Key::Esc), Effect::Redraw);
        assert_eq!(a.asking, None);
    }

    #[test]
    fn results_carry_messages_reloads_selection_and_things_to_open() {
        let mut a = list_app(&["a", "b", "c"]);
        let (effect, open) = a.apply(
            'x',
            Some("a".to_owned()),
            ActionResult {
                message: Some("已完成".to_owned()),
                reload: true,
                select: Some("c".to_owned()),
                open: Some("https://x".to_owned()),
                ask: None,
            },
        );
        assert_eq!(
            (effect, open.as_deref()),
            (Effect::Reload, Some("https://x"))
        );
        assert_eq!(a.message.as_deref(), Some("已完成"));
        // 重取之后选中要求的那一行
        a.loaded(Ok(PageView {
            heading: String::new(),
            note: String::new(),
            blocks: vec![Block::List {
                rows: vec![row("b"), row("c")],
            }],
        }));
        assert_eq!(a.selected_row().unwrap().id, "c");
        // 下一个键清掉提示
        a.handle(Key::Char('k'));
        assert_eq!(a.message, None);
    }

    #[test]
    fn after_a_row_goes_away_the_selection_stays_in_place() {
        let mut a = list_app(&["a", "b", "c"]);
        a.handle(Key::Char('j'));
        a.handle(Key::Char('j'));
        // c 完成了，列表剩两行：选中最后一行
        a.loaded(Ok(PageView {
            heading: String::new(),
            note: String::new(),
            blocks: vec![Block::List {
                rows: vec![row("a"), row("b")],
            }],
        }));
        assert_eq!(a.selected_row().unwrap().id, "b");
    }

    #[test]
    fn the_selected_row_is_marked_in_the_layout() {
        let a = list_app(&["a", "b"]);
        let Some(Ok(view)) = &a.view else { panic!() };
        let laid = layout_selected(view, 40, Some(1));
        let lines = text(&laid.lines);
        assert_eq!(&lines[2..], ["  条目 a", "› 条目 b"]);
        assert_eq!(laid.selected, Some(3));
    }
}
