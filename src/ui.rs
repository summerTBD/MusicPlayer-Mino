//! 终端界面渲染
//!
//! 策略：**静态内容只打印一次，动态内容原地重绘**。
//! - 标题、播放列表、快捷键等静态内容在启动时打印一次；
//! - 状态、当前歌曲、进度条、输入行等动态内容锚定在屏幕底部，
//!   每帧先擦除整行再重绘，避免整个界面反复输出导致页面不断向下滚动。

use std::io::{self, Write};
use std::time::Duration;

use crossterm::{
    cursor, queue,
    terminal::{self, Clear, ClearType},
};

use crate::state::PlayerState;

/// 动态状态区的位置与尺寸
pub struct StatusArea {
    /// 状态区起始行（0-based）
    pub row: u16,
    /// 状态区占用的行数（4 行：状态/当前/进度/输入）
    pub height: u16,
}

/// 一次性绘制静态部分（标题、列表、快捷键），并返回动态状态区位置。
///
/// 动态区固定在屏幕底部，列表太长时自动截断，避免与动态区重叠。
pub fn init_ui(state: &PlayerState) -> io::Result<StatusArea> {
    let (_, height) = terminal::size()?;
    let height = height.saturating_sub(1); // 最大行号（0-based）

    // 底部布局（从下往上）：
    //   height-8 ..= height-5 : 动态区（状态/当前/进度/输入）
    //   height-9              : 分隔线
    //   height-4              : 分隔线
    //   height-3 ..= height-1 : 快捷键
    let status_row = height.saturating_sub(8);

    let mut stdout = io::stdout().lock();
    queue!(stdout, Clear(ClearType::All), cursor::MoveTo(0, 0))?;

    // ---- 顶部静态：标题 + 列表（超出可视区部分截断） ----
    writeln!(stdout, "🎵 终端音乐播放器")?;
    writeln!(stdout, "{}", separator())?;
    writeln!(stdout, "📋 播放列表 (共 {} 首)", state.playlist.len())?;

    // 标题(1) + 分隔线(1) + 列表标题(1) 占 3 行，剩余空间给列表项；
    // 顶部内容不能越过 height-9 的分隔线，若列表被截断还要预留 1 行显示提示。
    let max_items = height.saturating_sub(13) as usize;
    let shown = state.playlist.len().min(max_items);
    for (i, path) in state.playlist.iter().take(shown).enumerate() {
        let name = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("未知文件");
        writeln!(stdout, "  {:>2}. {}", i + 1, name)?;
    }
    if state.playlist.len() > shown {
        writeln!(stdout, "  … 还有 {} 首未显示", state.playlist.len() - shown)?;
    }
    stdout.flush()?;

    // ---- 底部静态：分隔线 + 快捷键 ----
    let mut stdout = io::stdout().lock();
    queue!(stdout, cursor::MoveTo(0, status_row.saturating_sub(1)))?;
    write!(stdout, "{}", separator())?;
    queue!(stdout, cursor::MoveTo(0, height.saturating_sub(4)))?;
    write!(stdout, "{}", separator())?;
    queue!(stdout, cursor::MoveTo(0, height.saturating_sub(3)))?;
    write!(stdout, "⌨️  快捷键:")?;
    queue!(stdout, cursor::MoveTo(0, height.saturating_sub(2)))?;
    write!(
        stdout,
        "  数字+回车 → 播放指定歌曲 | n → 下一首 | p → 上一首"
    )?;
    queue!(stdout, cursor::MoveTo(0, height.saturating_sub(1)))?;
    write!(stdout, "  空格 → 暂停/继续 | m → 切换播放模式 | q → 退出")?;
    stdout.flush()?;

    Ok(StatusArea {
        row: status_row,
        height: 4,
    })
}

/// 原地重绘动态状态区（状态、当前歌曲、进度条、输入行）。
///
/// 每行先擦除再写入，不会在屏幕上累积残留内容。
pub fn update_status(state: &PlayerState, area: &StatusArea) -> io::Result<()> {
    let mut stdout = io::stdout().lock();

    // 状态行
    let status_text = match (state.current_index, state.is_paused) {
        (None, _) => "⏹ 停止",
        (Some(_), true) => "⏸ 暂停",
        (Some(_), false) => "▶ 播放中",
    };
    draw_line(
        &mut stdout,
        area.row,
        &format!(
            "状态: {} | 模式: {}",
            status_text,
            state.play_mode.to_display_str()
        ),
    )?;

    // 当前歌曲
    let id = match state.current_index {
        Some(idx) => idx + 1,
        None => 0,
    };
    draw_line(
        &mut stdout,
        area.row + 1,
        &format!("当前:第{}首 {}", id, state.current_song_name()),
    )?;

    // 进度条（使用 ASCII `#`/`-` 保证任意终端都能正常渲染；
    // 条宽 30，加上时间信息总宽约 74 列，避免在 80 列终端上换行导致重绘错乱）
    let progress = if state.current_index.is_some() && state.total_duration > Duration::ZERO {
        let total = state.total_duration.as_secs_f32();
        let current = state.estimated_position.as_secs_f32().min(total);
        let percent = current / total;
        let bar_width = 30;
        let filled = (percent * bar_width as f32).round() as usize;
        let empty = bar_width - filled;
        format!(
            "[{}{}] {:02}:{:02} / {:02}:{:02}",
            "#".repeat(filled),
            "-".repeat(empty),
            (current / 60.0) as u32,
            (current % 60.0) as u32,
            (total / 60.0) as u32,
            (total % 60.0) as u32
        )
    } else {
        format!("[{}] --:-- / --:--", "-".repeat(30))
    };
    draw_line(&mut stdout, area.row + 2, &progress)?;

    // 输入行
    draw_line(
        &mut stdout,
        area.row + 3,
        &format!("输入: {}", state.input_buffer),
    )?;

    stdout.flush()
}

/// 移到指定行，先擦除整行再写入新内容
fn draw_line(stdout: &mut impl Write, row: u16, text: &str) -> io::Result<()> {
    queue!(
        stdout,
        cursor::MoveTo(0, row),
        Clear(ClearType::UntilNewLine)
    )?;
    write!(stdout, "{text}")?;
    Ok(())
}

/// 生成分隔线
fn separator() -> String {
    "─".repeat(55)
}
