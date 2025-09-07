use std::sync::{Arc, mpsc};
use std::thread;
use std::time::{Duration, Instant};

use rodio::{Decoder, OutputStream, Sink, Source};
use slint::SharedString;

slint::include_modules!();

// ui --> backend
enum PlayerCommand {
    Play(String),
    Pause,
}

// backend --> ui
#[derive(Clone)]
struct PlayerState {
    progress: f32,
    duration: f32,
    paused: bool,
}

fn main() {
    let ui = MainWindow::new().unwrap();

    let (tx, rx) = mpsc::channel::<PlayerCommand>();
    let (progress_tx, progress_rx) = mpsc::channel::<PlayerState>();

    // 播放线程
    thread::spawn(move || {
        let (_stream, handle) = OutputStream::try_default().unwrap();
        let mut sink: Arc<Sink> = Arc::new(Sink::try_new(&handle).unwrap());
        let mut start_time;
        let mut duration;

        while let Ok(cmd) = rx.recv() {
            match cmd {
                PlayerCommand::Play(path) => {
                    if let Ok(file) = std::fs::File::open(&path) {
                        if let Ok(source) = Decoder::new(std::io::BufReader::new(file)) {
                            duration = source
                                .total_duration()
                                .map(|d| d.as_secs_f32())
                                .unwrap_or(0.0);

                            sink.stop();
                            sink = Arc::new(Sink::try_new(&handle).unwrap());
                            sink.append(source);

                            start_time = Instant::now();
                            sink.play();

                            // 启动进度上报线程
                            let sink_ref = Arc::clone(&sink);
                            let tx_clone = progress_tx.clone();
                            thread::spawn(move || {
                                let mut elapsed = start_time.elapsed().as_secs_f32();
                                let dura = 1000.;
                                loop {
                                    if sink_ref.empty() {
                                        println!("Play end!");
                                        break;
                                    }
                                    if !sink_ref.is_paused() {
                                        elapsed += dura / 1000.0;
                                    }
                                    let _ = tx_clone.send(PlayerState {
                                        progress: elapsed.min(duration),
                                        duration,
                                        paused: sink_ref.is_paused(),
                                    });
                                    thread::sleep(Duration::from_millis(dura as u64));
                                }
                            });
                        }
                    }
                }
                PlayerCommand::Pause => {
                    if sink.is_paused() {
                        sink.play();
                    } else {
                        sink.pause();
                    }
                }
            }
        }
    });

    // UI 触发事件
    {
        let tx = tx.clone();
        ui.on_play(move |path: SharedString| {
            tx.send(PlayerCommand::Play(path.to_string())).unwrap();
        });
    }
    {
        let tx = tx.clone();
        ui.on_pause(move || {
            tx.send(PlayerCommand::Pause).unwrap();
        });
    }

    // UI 定时刷新进度条
    let ui_weak = ui.as_weak();
    let timer = slint::Timer::default();
    timer.start(
        slint::TimerMode::Repeated,
        Duration::from_millis(500),
        move || {
            if let Ok(state) = progress_rx.try_recv() {
                if let Some(ui) = ui_weak.upgrade() {
                    ui.set_progress(state.progress);
                    ui.set_duration(state.duration.max(1.0));
                    ui.set_status(SharedString::from(
                        state.paused.then(|| "Paused").unwrap_or("Playing"),
                    ));
                }
            }
        },
    );

    ui.run().unwrap();
}
