//! 音频播放后台线程（基于 rodio 0.22 的 Player + MixerDeviceSink API）

use std::fs::File;
use std::io::BufReader;
use std::path::Path;
use std::sync::{Arc, Mutex, mpsc};
use std::thread;
use std::time::Duration;

use rodio::cpal;
use rodio::cpal::traits::{DeviceTrait, HostTrait};
use rodio::{Decoder, DeviceSinkBuilder, MixerDeviceSink, Player, Source};

use crate::config::UI_REFRESH_MS;
use crate::state::{Command, PlayMode, PlayerState};

/// 解码后的音频源类型（`BufReader<File>` 可随机访问，供解码器流式读取）
pub type DecodedSource = Decoder<BufReader<File>>;

/// 打开当前默认输出设备，并建立绑定到它的播放器。
///
/// 返回的设备句柄（`MixerDeviceSink`）**必须一直存活**，它内部握着 cpal 的
/// 音频流，一旦被丢弃播放就停了。句柄绑定的是「打开这一刻」的默认端点，
/// 所以端点换了（蓝牙耳机断电重连、切换输出设备…）之后必须重新调用本函数，
/// 旧句柄不会自己跟到新端点上去 —— 那正是「明明在播放却没声音」的原因。
///
/// 打不开默认设备时按 rodio 的降级顺序再试：换采样配置 → 换其它输出设备
/// （耳机刚断的那一瞬间，默认端点可能正处在不可用状态）。
fn open_output(
    tx: &mpsc::Sender<Command>,
) -> Result<(MixerDeviceSink, Player), Box<dyn std::error::Error>> {
    // cpal 默认的错误回调是 `eprintln!`，会在 ratatui 的备用屏幕上直接打字，
    // 而 ratatui 只重画「变化过」的格子，那行字会永久留在界面上。
    // 这里改成投回自己的命令通道，由界面按自己的方式画。
    let on_error = {
        let tx = tx.clone();
        move |err: cpal::StreamError| {
            let _ = tx.send(Command::StreamError(err.to_string()));
        }
    };

    let mut sink_handle = DeviceSinkBuilder::from_default_device()
        .and_then(|builder| builder.with_error_callback(on_error.clone()).open_stream())
        .or_else(|original_err| {
            let Ok(devices) = cpal::default_host().output_devices() else {
                return Err(original_err);
            };
            devices
                // 跳过占位的 null 驱动设备，它们放不出声音
                .filter(|device| {
                    device
                        .description()
                        .map(|desc| desc.driver().is_some_and(|driver| driver != "null"))
                        .unwrap_or(false)
                })
                .find_map(|device| {
                    DeviceSinkBuilder::from_device(device)
                        .ok()?
                        .with_error_callback(on_error.clone())
                        .open_sink_or_fallback()
                        .ok()
                })
                .ok_or(original_err)
        })?;

    // 关闭设备句柄被丢弃时的默认日志输出，避免噪音混入界面
    sink_handle.log_on_drop(false);
    let player = Player::connect_new(sink_handle.mixer());
    Ok((sink_handle, player))
}

/// 把当前曲目重新放回队列，并从刷新前的进度续播。
///
/// 没有曲目在播时只重建设备、不主动出声：免得「停止」状态被刷新键意外唤醒。
fn resume_current(player: &Player, state: &Arc<Mutex<PlayerState>>) {
    let (index, position, paused) = {
        let state = state.lock().unwrap();
        (
            state.current_index,
            state.estimated_position,
            state.is_paused,
        )
    };
    let Some(index) = index else { return };

    let path = state.lock().unwrap().playlist[index].clone();
    let Ok((source, duration)) = load_audio(&path) else {
        eprintln!("重新加载失败: {}", path.display());
        return;
    };

    start_track(player, source, duration, state, Some(index));

    // 进度已经跑到末尾（或读到脏数据）时从头放，免得刚进队列就触发下一首
    let resume_at = if position < duration {
        position
    } else {
        Duration::ZERO
    };
    if !resume_at.is_zero() && player.try_seek(resume_at).is_ok() {
        state.lock().unwrap().estimated_position = resume_at;
    }

    // 刷新前是暂停态就保持暂停，别把用户的暂停键按掉
    if paused {
        player.pause();
        state.lock().unwrap().is_paused = true;
    }
}

/// 音频后台线程：负责解码、播放、暂停、停止与自动切歌。
///
/// 需要同时传入 `rx`（接收主线程命令）与 `tx`（自动切歌时向自己发送命令）。
pub fn audio_thread(
    rx: mpsc::Receiver<Command>,
    tx: mpsc::Sender<Command>,
    state: Arc<Mutex<PlayerState>>,
) -> Result<(), Box<dyn std::error::Error>> {
    // 打开默认音频设备并建立播放器（句柄必须保持存活，播放依赖它）
    let (mut sink_handle, mut player) = open_output(&tx)?;
    // 音量以共享状态为唯一来源，这里不再自带一份拷贝
    player.set_volume(state.lock().unwrap().volume);

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

            Ok(Command::SetVolume(volume)) => {
                // 越界值不直达混音器：模型说多少就是多少
                let volume = volume.clamp(0.0, 1.0);
                player.set_volume(volume);
                state.lock().unwrap().volume = volume;
            }

            Ok(Command::StreamError(message)) => {
                // 设备被系统摘掉（耳机断电等）时 cpal 会走到这里。
                // 留着这条信息，界面据此提示用户按 `r` 重连
                state.lock().unwrap().stream_error = Some(message);
            }

            Ok(Command::RefreshDevice) => {
                // 重新打开「当前」默认设备：耳机断电重连后默认端点已经换了一个，
                // 旧句柄上的声音全进了空气。开不成新设备时保留旧的，不做破坏性改动。
                match open_output(&tx) {
                    Ok((new_sink, new_player)) => {
                        new_player.set_volume(state.lock().unwrap().volume);
                        // 先停掉发往旧设备的声音（丢弃旧播放器），再释放旧 cpal 流
                        drop(std::mem::replace(&mut player, new_player));
                        drop(std::mem::replace(&mut sink_handle, new_sink));

                        // 设备已经重建，旧的断开告警随之作废
                        state.lock().unwrap().stream_error = None;
                        resume_current(&player, &state);
                    }
                    Err(e) => eprintln!("重新打开音频设备失败: {e}"),
                }
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
