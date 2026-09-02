//! 验证 rodio 0.22 的 `Player::get_pos()` 是否随播放推进。
//!
//! 需要真实音频设备；运行：`cargo test --test progress_test -- --nocapture`

use std::time::{Duration, Instant};

use rodio::{DeviceSinkBuilder, Player, Source};

#[test]
fn get_pos_advances() {
    let mut sink = DeviceSinkBuilder::open_default_sink().expect("打开音频设备失败");
    sink.log_on_drop(false);
    let player = Player::connect_new(sink.mixer());

    let source = rodio::source::SineWave::new(440.0)
        .take_duration(Duration::from_secs(5))
        .amplify(0.1);
    player.append(source);
    player.play();

    let start = Instant::now();
    let mut last = Duration::ZERO;
    for _ in 0..10 {
        std::thread::sleep(Duration::from_millis(200));
        let pos = player.get_pos();
        eprintln!(
            "elapsed={:?} pos={:?} delta={:?} empty={}",
            start.elapsed(),
            pos,
            pos.saturating_sub(last),
            player.empty()
        );
        last = pos;
    }
    assert!(
        last >= Duration::from_millis(1500),
        "get_pos 没有推进，last={last:?}"
    );
}
