//! 终端音乐播放器核心库
//!
//! 模块划分：
//! - [`app`]：应用主体（`App` + `Message` + 事件循环）；
//! - [`ui`]：ratatui 渲染层（只读状态、无副作用）；
//! - [`state`]：跨线程共享的播放状态与命令定义；
//! - [`audio`]：音频后台线程（解码 / 播放 / 自动切歌）；
//! - [`music`]：音乐文件扫描；[`config`]：全局常量。

pub mod app;
pub mod audio;
pub mod config;
pub mod music;
pub mod state;
pub mod ui;

use crate::app::App;

/// 程序入口：创建应用并进入 ratatui 主循环。
///
/// 终端初始化/恢复、事件循环、命令分发都在 [`App`] 中，这里只做转发。
pub fn run() -> Result<(), Box<dyn std::error::Error>> {
    let mut app = App::new()?;
    app.run()?;
    println!("👋 播放器已退出");
    Ok(())
}
