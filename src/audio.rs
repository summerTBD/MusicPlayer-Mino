//! 音频播放后台线程（基于 rodio 0.22 的 Player + MixerDeviceSink API）

use std::fs::File;
use std::io::BufReader;
use std::path::Path;
use std::sync::{Arc, Mutex, mpsc};
use std::thread;
use std::time::Duration;

use rodio::{Decoder, DeviceSinkBuilder, Player, Source};

use crate::config::{DEFAULT_VOLUME, UI_REFRESH_MS};
use crate::state::{Command, PlayMode, PlayerState};

/// 解码后的音频源类型（`BufReader<File>` 可随机访问，供解码器流式读取）
pub type DecodedSource = Decoder<BufReader<File>>;

/// 音频后台线程：负责解码、播放、暂停、停止与自动切歌。
///
/// 需要同时传入 `rx`（接收主线程命令）与 `tx`（自动切歌时向自己发送命令）。
pub fn audio_thread(
    rx: mpsc::Receiver<Command>,
    tx: mpsc::Sender<Command>,
    state: Arc<Mutex<PlayerState>>,
) -> Result<(), Box<dyn std::error::Error>> {
    // 打开默认音频设备（句柄必须保持存活，播放依赖它）
    let mut sink_handle = DeviceSinkBuilder::open_default_sink()?;
    // 关闭设备句柄被丢弃时的默认日志输出，避免噪音混入界面
    sink_handle.log_on_drop(false);
    let player = Player::connect_new(sink_handle.mixer());
    player.set_volume(DEFAULT_VOLUME);

    loop {
        // 1. 非阻塞处理控制命令
        match rx.try_recv() {
            // 退出：停播放后结束循环。
            // （不再需要置一个 running 标志：主线程不再轮询它，
            //   而 recv 端断开时下面的 Disconnected 分支也会让循环退出）
            Ok(Command::Quit) => {
                player.stop();
                break;
            }

            Ok(Command::Play(index)) => {
                let path = {
                    let state = state.lock().unwrap();
                    if index >= state.playlist.len() {
                        continue;
                    }
                    state.playlist[index].clone()
                };
                match load_audio(&path) {
                    Ok((source, duration)) => {
                        start_track(&player, source, duration, &state, Some(index));
                    }
                    Err(e) => eprintln!("\n加载失败: {} - {}", path.display(), e),
                }
            }

            Ok(Command::Next) => {
                let (next_idx, should_stop) = {
                    let state = state.lock().unwrap();
                    let Some(current_idx) = state.current_index else {
                        continue;
                    };
                    match state.play_mode {
                        PlayMode::LoopAll => ((current_idx + 1) % state.playlist.len(), false),
                        _ if current_idx + 1 < state.playlist.len() => (current_idx + 1, false),
                        // 顺序播放到末尾：停止
                        _ => (0, true),
                    }
                };

                if should_stop {
                    player.stop();
                    let mut state = state.lock().unwrap();
                    state.current_index = None;
                    state.is_paused = false;
                    state.estimated_position = Duration::ZERO;
                    continue;
                }

                let path = state.lock().unwrap().playlist[next_idx].clone();
                if let Ok((source, duration)) = load_audio(&path) {
                    start_track(&player, source, duration, &state, Some(next_idx));
                }
            }

            Ok(Command::Prev) => {
                let prev_idx = {
                    let state = state.lock().unwrap();
                    let Some(current_idx) = state.current_index else {
                        continue;
                    };
                    if current_idx == 0 {
                        match state.play_mode {
                            PlayMode::LoopAll => state.playlist.len() - 1,
                            _ => 0,
                        }
                    } else {
                        current_idx - 1
                    }
                };

                let path = state.lock().unwrap().playlist[prev_idx].clone();
                if let Ok((source, duration)) = load_audio(&path) {
                    start_track(&player, source, duration, &state, Some(prev_idx));
                }
            }

            Ok(Command::TogglePause) => {
                let mut state = state.lock().unwrap();
                if state.is_paused {
                    player.play();
                    state.is_paused = false;
                } else {
                    player.pause();
                    state.is_paused = true;
                }
            }

            Ok(Command::SetMode(mode)) => {
                state.lock().unwrap().play_mode = mode;
            }

            Ok(Command::Stop) => {
                player.stop();
                let mut state = state.lock().unwrap();
                state.current_index = None;
                state.is_paused = false;
                state.estimated_position = Duration::ZERO;
            }

            Err(mpsc::TryRecvError::Empty) => {}
            Err(mpsc::TryRecvError::Disconnected) => break,
        }

        // 2. 更新播放进度（取自播放器内部计时，暂停时自动冻结）
        {
            let mut state = state.lock().unwrap();
            if state.current_index.is_some() {
                state.estimated_position = player.get_pos();
            }
        }

        // 3. 自动检测播放结束，触发下一首
        let should_advance = {
            let state = state.lock().unwrap();
            player.empty() && state.current_index.is_some()
        };

        if should_advance {
            let play_mode = state.lock().unwrap().play_mode;
            if play_mode == PlayMode::LoopSingle {
                let idx = state.lock().unwrap().current_index.unwrap();
                let path = state.lock().unwrap().playlist[idx].clone();
                if let Ok((source, duration)) = load_audio(&path) {
                    start_track(&player, source, duration, &state, Some(idx));
                }
            } else {
                // 其他模式：请求播放下一首
                let _ = tx.send(Command::Next);
            }
        }

        thread::sleep(Duration::from_millis(UI_REFRESH_MS));
    }

    Ok(())
}

/// 加载音频文件，返回解码器与总时长
pub fn load_audio(path: &Path) -> Result<(DecodedSource, Duration), Box<dyn std::error::Error>> {
    let file = File::open(path)?;
    let reader = BufReader::new(file);
    let source = Decoder::try_from(reader)?;
    let duration = source.total_duration().ok_or("无法读取音频时长")?;
    Ok((source, duration))
}

/// 启动播放一首新曲目并更新共享状态
fn start_track(
    player: &Player,
    source: impl Source + Send + 'static,
    duration: Duration,
    state: &Arc<Mutex<PlayerState>>,
    index: Option<usize>,
) {
    player.stop();
    player.append(source);
    player.play();

    let mut state = state.lock().unwrap();
    state.current_index = index;
    state.total_duration = duration;
    state.is_paused = false;
    state.estimated_position = Duration::ZERO;
}
