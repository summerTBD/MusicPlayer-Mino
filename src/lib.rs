//! 终端音乐播放器核心库
//!
//! 提供模块划分与 `run()` 入口，供 `main.rs` 调用。

pub mod audio;
pub mod config;
pub mod music;
pub mod state;
pub mod ui;

use std::io;
use std::path::Path;
use std::sync::{Arc, Mutex, mpsc};
use std::thread;
use std::time::Duration;

use crossterm::{cursor, event, execute, terminal};

use crate::audio::audio_thread;
use crate::config::{MUSIC_DIRECTORY, UI_REFRESH_MS};
use crate::music::collect_music_files;
use crate::state::{Command, PlayerState};
use crate::ui::{init_ui, update_status};

/// 程序入口：初始化终端、扫描音乐、启动音频线程并进入主循环。
pub fn run() -> Result<(), Box<dyn std::error::Error>> {
    // 初始化终端原始模式（捕获单键输入、关闭回显）
    terminal::enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, cursor::Hide)?;

    // Panic 时恢复终端，避免程序崩溃后终端异常
    let original_panic = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let _ = terminal::disable_raw_mode();
        let _ = execute!(io::stdout(), cursor::Show);
        original_panic(info);
    }));

    // 扫描音乐目录
    let music_dir = Path::new(MUSIC_DIRECTORY);
    let playlist = collect_music_files(music_dir)?;
    if playlist.is_empty() {
        execute!(stdout, cursor::Show)?;
        terminal::disable_raw_mode()?;
        eprintln!("❌ 目录 {} 中未找到支持的音频文件", music_dir.display());
        return Ok(());
    }

    // 初始化共享状态与命令通道
    let state = Arc::new(Mutex::new(PlayerState::new(playlist)));
    let (tx, rx) = mpsc::channel();

    // 启动音频后台线程（clone 一份发送端用于自动切歌）
    let state_clone = Arc::clone(&state);
    let tx_audio = tx.clone();
    thread::spawn(move || {
        if let Err(e) = audio_thread(rx, tx_audio, state_clone) {
            eprintln!("\n音频线程异常: {}", e);
        }
    });

    // 一次性绘制静态界面（标题、列表、快捷键），获取动态状态区位置
    let status_area = {
        let state = state.lock().unwrap();
        init_ui(&state)?
    };

    // 启动后自动播放第一首（若不需要可删除此行）
    tx.send(Command::Play(0))?;

    // 主循环：渲染 UI + 带超时的按键轮询（保证进度条实时刷新）
    loop {
        {
            let state = state.lock().unwrap();
            if !state.running {
                break;
            }
            update_status(&state, &status_area)?;
        }

        if event::poll(Duration::from_millis(UI_REFRESH_MS))?
            && let event::Event::Key(key) = event::read()?
            && key.kind == event::KeyEventKind::Press
        {
            let mut state = state.lock().unwrap();
            match key.code {
                event::KeyCode::Char('q' | 'Q') => {
                    tx.send(Command::Quit)?;
                    break;
                }
                event::KeyCode::Char('n' | 'N') => {
                    tx.send(Command::Next)?;
                }
                event::KeyCode::Char('p' | 'P') => {
                    tx.send(Command::Prev)?;
                }
                event::KeyCode::Char(' ') => {
                    tx.send(Command::TogglePause)?;
                }
                event::KeyCode::Char('m' | 'M') => {
                    let new_mode = state.play_mode.cycle();
                    tx.send(Command::SetMode(new_mode))?;
                }
                event::KeyCode::Char(c) if c.is_ascii_digit() => {
                    state.input_buffer.push(c);
                }
                event::KeyCode::Enter => {
                    if let Ok(num) = state.input_buffer.trim().parse::<usize>()
                        && num >= 1
                        && num <= state.playlist.len()
                    {
                        tx.send(Command::Play(num - 1))?; // 转 0-based 索引
                    }
                    state.input_buffer.clear();
                }
                event::KeyCode::Backspace => {
                    state.input_buffer.pop();
                }
                _ => {}
            }
        }
    }

    // 退出前稍作等待，让音频线程优雅停止
    thread::sleep(Duration::from_millis(50));

    // 恢复终端状态
    execute!(stdout, cursor::Show)?;
    terminal::disable_raw_mode()?;
    println!("\n👋 播放器已退出");

    Ok(())
}
