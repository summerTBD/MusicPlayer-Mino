//! 应用主体（TEA 风格：状态 + 消息 + update/view）
//!
//! 结构：
//! - [`App`]：持有共享的 [`PlayerState`]、命令发送端、UI 输入缓冲与退出标记；
//! - [`Message`]：把所有外部事件（按键）归一化成消息；
//! - [`App::update`]：**唯一**修改状态 / 发送命令的地方；
//! - [`App::view`]：只加锁读状态并交给 [`crate::ui::render`]，本身不改任何东西。

use std::io;
use std::path::Path;
use std::sync::{Arc, Mutex, mpsc};
use std::thread;
use std::time::Duration;

use ratatui::Frame;
use ratatui::crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind};

use crate::audio::audio_thread;
use crate::config::{MUSIC_DIRECTORY, UI_REFRESH_MS};
use crate::music::collect_music_files;
use crate::state::{Command, PlayerState};
use crate::ui;

/// 应用消息：把「发生了什么」与「怎么处理」解耦
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Message {
    /// 退出程序
    Quit,
    /// 下一首
    Next,
    /// 上一首
    Prev,
    /// 暂停 / 继续
    TogglePause,
    /// 切换播放模式
    CycleMode,
    /// 数字键：追加到输入缓冲
    Input(char),
    /// 退格：删除输入缓冲最后一个字符
    Backspace,
    /// 回车：提交输入缓冲（按序号选歌）
    SubmitInput,
    /// 列表上滚一行（同时关闭自动跟随）
    PlayListScrollUp,
    /// 列表下滚一行（同时关闭自动跟随）
    PlayListScrollDown,
    /// 光标上移一行（首次按下只把光标落在起点，不移动）
    CursorScrollUp,
    /// 光标下移一行
    CursorScrollDown,
}

impl Message {
    /// 把按键事件映射为消息；`None` 表示该键无需处理
    pub fn from_key(key: KeyEvent) -> Option<Self> {
        match key.code {
            KeyCode::Char('q' | 'Q') => Some(Self::Quit),
            KeyCode::Char('n' | 'N') => Some(Self::Next),
            KeyCode::Char('p' | 'P') => Some(Self::Prev),
            KeyCode::Char(' ') => Some(Self::TogglePause),
            KeyCode::Char('m' | 'M') => Some(Self::CycleMode),
            KeyCode::Char(c) if c.is_ascii_digit() => Some(Self::Input(c)),
            KeyCode::Enter => Some(Self::SubmitInput),
            KeyCode::Backspace => Some(Self::Backspace),
            KeyCode::Up => Some(Self::PlayListScrollUp),
            KeyCode::Down => Some(Self::PlayListScrollDown),
            KeyCode::Char('k') => Some(Self::CursorScrollUp),
            KeyCode::Char('j') => Some(Self::CursorScrollDown),
            _ => None,
        }
    }
}

/// 应用主体：持有状态与通信通道，驱动「事件 → 消息 → 更新 → 渲染」循环
pub struct App {
    /// 与音频线程共享的播放器状态
    state: Arc<Mutex<PlayerState>>,
    /// 主线程 → 音频线程的命令通道
    tx: mpsc::Sender<Command>,
    /// 视图状态（输入缓冲、列表窗口）：只属于主线程，不跨线程也不进模型
    view: ui::ViewState,
    /// 是否请求退出
    should_quit: bool,
}

impl App {
    /// 扫描音乐目录、建立共享状态并启动音频后台线程
    pub fn new() -> Result<Self, Box<dyn std::error::Error>> {
        let music_dir = Path::new(MUSIC_DIRECTORY);
        let playlist = collect_music_files(music_dir)?;
        if playlist.is_empty() {
            return Err(format!("目录 {} 中未找到支持的音频文件", music_dir.display()).into());
        }

        let state = Arc::new(Mutex::new(PlayerState::new(playlist)));
        let (tx, rx) = mpsc::channel();

        // 音频线程：clone 一份发送端用于自动切歌
        let audio_state = Arc::clone(&state);
        let audio_tx = tx.clone();
        thread::spawn(move || {
            if let Err(e) = audio_thread(rx, audio_tx, audio_state) {
                eprintln!("音频线程异常: {e}");
            }
        });

        Ok(Self {
            state,
            tx,
            view: ui::ViewState::new(),
            should_quit: false,
        })
    }

    /// 进入 ratatui 主循环：初始化终端 → 循环 → 退出时恢复终端
    pub fn run(&mut self) -> io::Result<()> {
        // `ratatui::init()` 会开启原始模式、进入备用屏幕并设置 panic 时自动恢复终端
        let mut terminal = ratatui::init();
        let result = self.main_loop(&mut terminal);
        ratatui::restore();
        result
    }

    /// 主循环：每帧渲染 + 带超时的按键轮询（超时用于持续刷新进度条）
    fn main_loop(&mut self, terminal: &mut ratatui::DefaultTerminal) -> io::Result<()> {
        // 启动后自动播放第一首（不需要可删除）
        let _ = self.tx.send(Command::Play(0));

        while !self.should_quit {
            terminal.draw(|frame| self.view(frame))?;

            if event::poll(Duration::from_millis(UI_REFRESH_MS))?
                && let Event::Key(key) = event::read()?
                // Windows 终端一次按键会送 Press + Release，必须过滤，否则输入会重复
                && key.kind == KeyEventKind::Press
                && let Some(message) = Message::from_key(key)
            {
                self.update(message);
            }
        }

        // 通知音频线程停止，并留一点收尾时间
        let _ = self.tx.send(Command::Quit);
        thread::sleep(Duration::from_millis(50));
        Ok(())
    }

    /// 处理消息：状态变更与命令发送都集中在这里
    ///
    /// 约定：只有**这里**会修改模型状态；视图状态（输入、窗口）也在这里响应事件，
    /// 唯一的例外是窗口上界的夹紧（渲染时才知道可视行数，见 `ui::render_playlist`）。
    fn update(&mut self, message: Message) {
        match message {
            Message::Quit => self.should_quit = true,
            Message::Next => {
                self.follow_current(); // 切歌 = 恢复自动跟随（并收起光标）
                let _ = self.tx.send(Command::Next);
            }
            Message::Prev => {
                self.follow_current();
                let _ = self.tx.send(Command::Prev);
            }
            Message::TogglePause => {
                let _ = self.tx.send(Command::TogglePause);
            }
            Message::CycleMode => {
                let new_mode = self.state.lock().unwrap().play_mode.cycle();
                let _ = self.tx.send(Command::SetMode(new_mode));
            }
            Message::Input(c) => {
                self.view.input_error = None; // 重新输入即消掉上一次的报错
                self.view.input.push(c);
            }
            Message::Backspace => {
                self.view.input_error = None;
                self.view.input.pop();
            }
            Message::SubmitInput => {
                let len = self.state.lock().unwrap().playlist.len();
                let typed = self.view.input.trim().to_string();
                match typed.parse::<usize>() {
                    // 合法序号：转 0-based 索引直接播
                    Ok(num) if (1..=len).contains(&num) => self.play_index(num - 1),
                    // 直接回车（空输入）：有光标就播放光标所在行（j/k 选中 → 回车播放）
                    _ if typed.is_empty() => {
                        if let Some(index) = self.view.cursor {
                            self.play_index(index);
                        }
                    }
                    // 非法序号：留下可见反馈，而不是静默丢弃
                    _ => self.view.input_error = Some(typed),
                }
                self.view.input.clear();
            }
            // 手动滚动窗口 = 纯「看一看」，不选歌，所以顺手收起光标
            Message::PlayListScrollUp => {
                self.view.follow = false;
                self.view.cursor = None;
                self.view.offset = self.view.offset.saturating_sub(1);
            }
            Message::PlayListScrollDown => {
                self.view.follow = false;
                self.view.cursor = None;
                // 上界不在这里夹：只有渲染时才知道可视行数
                self.view.offset = self.view.offset.saturating_add(1);
            }

            Message::CursorScrollUp => self.move_cursor(-1),
            Message::CursorScrollDown => self.move_cursor(1),
        }
    }

    /// 回到「跟随当前歌曲」模式：窗口跟着歌走，光标收起（选歌 / 切歌都走这里）
    fn follow_current(&mut self) {
        self.view.follow = true;
        self.view.cursor = None;
    }

    /// 播放指定索引（0-based）：选歌即回到跟随模式
    fn play_index(&mut self, index: usize) {
        self.follow_current();
        let _ = self.tx.send(Command::Play(index));
    }

    /// 移动列表光标（j/k）
    ///
    /// 光标是**选择**，窗口是**视野**：这里只动光标，「让光标可见」的活儿
    /// 交给渲染层（只有渲染时才知道可视行数）。
    fn move_cursor(&mut self, delta: isize) {
        let (len, playing) = {
            let state = self.state.lock().unwrap();
            (state.playlist.len(), state.current_index)
        };
        if len == 0 {
            return;
        }

        // 起点：已有光标就从光标走；否则落在「当前歌曲」或「窗口顶端」上
        let start = self
            .view
            .cursor
            .or(playing)
            .unwrap_or(self.view.offset)
            .min(len - 1);
        let next = match self.view.cursor {
            None => start, // 首次按下只落点，免得一下跳两行
            Some(cur) if delta < 0 => cur.saturating_sub(1),
            Some(cur) => (cur + 1).min(len - 1),
        };

        self.view.follow = false; // 用户接管 → 摘掉自动跟随
        self.view.cursor = Some(next);
    }

    /// 渲染一帧：加锁读取共享模型后交给渲染层
    ///
    /// 模型只读；`view` 可变，但渲染只会夹紧视图自己的窗口边界。
    fn view(&mut self, frame: &mut Frame) {
        let state = self.state.lock().unwrap();
        ui::render(frame, &state, &mut self.view);
    }
}
