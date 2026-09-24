//! 终端界面渲染（ratatui）
//!
//! 只负责「把状态画出来」：不修改状态、不发送命令、不关心事件。
//! 布局自上而下：标题 / 播放列表 / 播放状态（含进度条与输入行）/ 快捷键。
//!
//! 屏幕输出、清屏、光标、双缓冲全部交给 ratatui 的 `Terminal`，
//! 因此不再需要旧版那套「静态只打印一次 + 手动定位擦行」的技巧。

use std::time::Duration;

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Gauge, List, ListItem, ListState, Paragraph};

use crate::state::PlayerState;

/// 视图状态：只影响「怎么画」，与音频线程无关。
///
/// 单独成类型，是为了让 [`render`] 的入参一眼分清两类东西：
/// - [`PlayerState`]：**模型**，跳线程共享（音频线程写，这里只读）；
/// - `ViewState`：**视图**，只属于主线程（事件改，渲染时可能夹紧边界）。
#[derive(Debug)]
pub struct ViewState {
    /// 数字输入缓冲
    pub input: String,
    /// 最近一次被拒绝的输入（用于红字提示），`None` 表示无错误
    pub input_error: Option<String>,
    /// 列表窗口第一项的下标
    pub offset: usize,
    /// 列表光标（j/k 移动、回车播放）；`None` 表示没有光标
    ///
    /// 不变式：`follow == true` 时恒为 `None` —— 自动跟随模式下「当前歌曲」就是唯一焦点。
    pub cursor: Option<usize>,
    /// 是否让窗口自动跟随当前歌曲（用户一滚动/一移光标就关，一选歌就恢复）
    pub follow: bool,
}

impl ViewState {
    pub fn new() -> Self {
        Self {
            input: String::new(),
            input_error: None,
            offset: 0,
            cursor: None,
            follow: true,
        }
    }
}

impl Default for ViewState {
    fn default() -> Self {
        Self::new()
    }
}

/// 渲染一帧。
///
/// 模型（`state`）只读；`view` 是可变的，但只会动视图自己的东西
/// （把窗口起点夹紧到合法范围），不会反向影响模型。
pub fn render(frame: &mut Frame, state: &PlayerState, view: &mut ViewState) {
    let [title_area, list_area, status_area, help_area] = Layout::vertical([
        Constraint::Length(3), // 标题（带边框）
        Constraint::Min(5),    // 播放列表：占满剩余空间
        Constraint::Length(6), // 状态 + 进度条 + 输入（带边框，内部 4 行）
        Constraint::Length(5), // 快捷键（带边框，内部 3 行）
    ])
    .areas(frame.area());

    render_title(frame, state, title_area);
    render_playlist(frame, state, view, list_area);
    render_status(frame, state, view, status_area);
    render_help(frame, help_area);
}

/// 顶部标题栏
fn render_title(frame: &mut Frame, state: &PlayerState, area: Rect) {
    let title = Line::from(vec![
        Span::styled(
            "🎵 终端音乐播放器",
            Style::new().fg(Color::Cyan).add_modifier(Modifier::BOLD),
        ),
        Span::raw(format!("   共 {} 首", state.playlist.len())),
    ]);
    let paragraph = Paragraph::new(title).block(bordered("", Style::new().fg(Color::DarkGray)));
    frame.render_widget(paragraph, area);
}

/// 播放列表：当前歌曲用 `▶` + 绿色加粗，光标另叠一层反色
///
/// `▶` 与光标都是逐项判断画出来的，跟「窗口在哪」无关；
/// 窗口起点则完全由 [`ViewState`] 决定（见下方 `follow` 分支）。
fn render_playlist(frame: &mut Frame, state: &PlayerState, view: &mut ViewState, area: Rect) {
    let items: Vec<ListItem> = state
        .playlist
        .iter()
        .enumerate()
        .map(|(idx, path)| {
            let name = path
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("未知文件");
            let is_current = state.current_index == Some(idx);
            let is_cursor = view.cursor == Some(idx);
            let marker = if is_current { "▶ " } else { "  " };
            // 当前播放 → 绿色加粗；光标 → 再叠一层反色（同一行时就是反色绿）
            let style = if is_current {
                Style::new().fg(Color::Green).add_modifier(Modifier::BOLD)
            } else {
                Style::new()
            };
            let style = if is_cursor {
                style.add_modifier(Modifier::REVERSED)
            } else {
                style
            };
            // 行号始终是歌单中的真实序号（1-based），键盘数字选歌才能对应得上
            ListItem::new(Line::from(format!("{marker}{:>3}. {name}", idx + 1))).style(style)
        })
        .collect();

    let border_style = Style::new().fg(Color::DarkGray);
    let len = state.playlist.len();
    // 标题顺带提示当前处于哪种「非跟随」模式
    let suffix = match view.cursor {
        Some(cursor) => format!(" — 光标 第 {} 首", cursor + 1),
        None if !view.follow => " — 手动滚动".to_string(),
        None => String::new(),
    };
    let block = Block::bordered()
        .title(format!(" 📋 播放列表 (共 {len} 首){suffix} "))
        .border_style(border_style);

    // 可视行数 = 区域高度 - 上下边框（与 List 内部拿到的 list_height 一致）
    let visible = area.height.saturating_sub(2) as usize;
    if visible > 0 {
        if view.follow {
            // 自动跟随：每帧由当前歌曲推导窗口起点（尽量让它居中）
            view.offset = match state.current_index {
                Some(cur) => cur
                    .saturating_sub(visible / 2)
                    .min(len.saturating_sub(visible)),
                None => 0,
            };
        } else {
            // 光标动过之后，用「最小必要的位移」把它纳入视野
            if let Some(cursor) = view.cursor {
                if cursor < view.offset {
                    view.offset = cursor;
                } else if cursor >= view.offset + visible {
                    view.offset = cursor + 1 - visible;
                }
            }
            // 最后夹紧上界（只有渲染时才知道可视行数）
            view.offset = view.offset.min(len.saturating_sub(visible));
        }
    }

    // 当前歌曲已用 `▶` + 颜色标注，所以不用 List 自带的选中高亮
    let list = List::new(items).block(block);
    let mut list_state = ListState::default().with_offset(view.offset);
    frame.render_stateful_widget(list, area, &mut list_state);
}

/// 播放状态：状态行、当前歌曲、进度条、输入行
fn render_status(frame: &mut Frame, state: &PlayerState, view: &ViewState, area: Rect) {
    let border_style = Style::new().fg(Color::DarkGray);
    let block = Block::bordered()
        .title(" 播放状态 ")
        .border_style(border_style);
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let [info_area, gauge_area, input_area] = Layout::vertical([
        Constraint::Length(2), // 状态 + 当前歌曲
        Constraint::Length(1), // 进度条
        Constraint::Length(1), // 输入行
    ])
    .areas(inner);

    // 状态行 + 当前歌曲（两行）
    let status_span = match (state.current_index, state.is_paused) {
        (None, _) => Span::styled("⏹ 停止", Style::new().fg(Color::DarkGray)),
        (Some(_), true) => Span::styled("⏸ 暂停", Style::new().fg(Color::Yellow)),
        (Some(_), false) => Span::styled("▶ 播放中", Style::new().fg(Color::Green)),
    };
    let current_no = state.current_index.map(|i| i + 1).unwrap_or(0);
    let info = Paragraph::new(vec![
        Line::from(vec![
            Span::raw("状态: "),
            status_span,
            Span::raw(format!("  |  模式: {}", state.play_mode.to_display_str())),
        ]),
        Line::from(format!(
            "当前: 第{current_no}首 {}",
            state.current_song_name()
        )),
    ]);
    frame.render_widget(info, info_area);

    // 进度条：宽度随终端自适应，不再需要手算条形宽度
    let (ratio, label) = progress(state);
    let gauge = Gauge::default()
        .ratio(ratio)
        .label(label)
        .gauge_style(Style::new().fg(Color::Green));
    frame.render_widget(gauge, gauge_area);

    // 输入行：三态 —— 报错（红）/ 空（灰提示）/ 输入中（黄 + 光标块）
    let input_line = if let Some(rejected) = &view.input_error {
        Line::styled(
            format!(
                "输入: {rejected} ✗ 序号超出范围（1-{}）",
                state.playlist.len()
            ),
            Style::new().fg(Color::Red).add_modifier(Modifier::BOLD),
        )
    } else if view.input.is_empty() {
        Line::styled(
            "输入: 数字 + 回车 → 跳转歌曲",
            Style::new().fg(Color::DarkGray),
        )
    } else {
        Line::styled(
            format!("输入: {}▏", view.input),
            Style::new().fg(Color::Yellow).add_modifier(Modifier::BOLD),
        )
    };
    frame.render_widget(Paragraph::new(input_line), input_area);
}

/// 计算进度条的比例与时间标签
fn progress(state: &PlayerState) -> (f64, String) {
    if state.current_index.is_none() || state.total_duration == Duration::ZERO {
        return (0.0, "--:-- / --:--".to_string());
    }
    let total = state.total_duration.as_secs_f64();
    let current = state.estimated_position.as_secs_f64().min(total);
    let ratio = if total > 0.0 {
        (current / total).clamp(0.0, 1.0)
    } else {
        0.0
    };
    (
        ratio,
        format!("{} / {}", format_time(current), format_time(total)),
    )
}

/// 秒数格式化为 `mm:ss`（超过一小时则为 `h:mm:ss`）
fn format_time(secs: f64) -> String {
    let total = secs.max(0.0) as u64;
    let (h, m, s) = (total / 3600, (total % 3600) / 60, total % 60);
    if h > 0 {
        format!("{h}:{m:02}:{s:02}")
    } else {
        format!("{m:02}:{s:02}")
    }
}

/// 底部快捷键提示
fn render_help(frame: &mut Frame, area: Rect) {
    let style = Style::new().fg(Color::DarkGray);
    let help = Paragraph::new(vec![
        Line::from(" 数字+回车 → 播放指定歌曲 | n → 下一首 | p → 上一首"),
        Line::from(" j/k → 移动光标（回车播放） | ↑/↓ → 滚动窗口"),
        Line::from(" 空格 → 暂停/继续 | m → 切换播放模式 | q → 退出"),
    ])
    .style(style)
    .block(bordered(" ⌨️ 快捷键 ", style));
    frame.render_widget(help, area);
}

/// 带边框的区块（统一边框颜色），`title` 为空时不显示标题
fn bordered<'a>(title: impl Into<Line<'a>>, style: Style) -> Block<'a> {
    Block::bordered().title(title).border_style(style)
}
