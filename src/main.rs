use std::sync::{Arc, Mutex, mpsc};
use std::thread;
use std::time::Duration;

use rodio::{Decoder, OutputStream, Sink, Source};
use slint::SharedString;

slint::include_modules!();

// ui --> backend
enum PlayerCommand {
    Play(String),        // 从头播放某个音频文件
    Pause,               // 暂停/继续播放
    ChangeProgress(f32), // 拖拽进度条
}

// backend --> ui
#[derive(Clone, Debug)]
struct PlayerState {
    progress: f32,             // 当前音频播放进度 (秒)
    duration: f32,             // 当前播放音频总时长 (秒)
    paused: bool,              // 是否处于暂停状态
    progress_info_str: String, // 进度信息字符串
}

fn make_time_str(secs: f32) -> String {
    let total_secs = secs as u32;
    let minutes = total_secs / 60;
    let seconds = total_secs % 60;
    format!("{:02}:{:02}", minutes, seconds)
}

fn main() {
    let ui = MainWindow::new().unwrap();

    let (tx, rx) = mpsc::channel::<PlayerCommand>();
    let (progress_tx, progress_rx) = mpsc::channel::<PlayerState>();

    // 播放线程
    thread::spawn(move || {
        let (_stream, handle) = OutputStream::try_default().unwrap();
        let mut sink: Arc<Sink> = Arc::new(Sink::try_new(&handle).unwrap());
        let progress = Arc::new(Mutex::new(0.0f32));
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

                            sink.play();

                            // 启动进度上报线程
                            let sink_ref = sink.clone();
                            let tx_clone = progress_tx.clone();
                            let progress = progress.clone();
                            thread::spawn(move || {
                                let dura = 1000.;
                                loop {
                                    if sink_ref.empty() {
                                        println!("Play end!");
                                        break;
                                    }
                                    {
                                        let mut _progress = progress.lock().unwrap();
                                        if !sink_ref.is_paused() {
                                            *_progress += dura / 1000.0;
                                        }
                                        let _ = tx_clone.send(PlayerState {
                                            progress: _progress.min(duration),
                                            duration: duration,
                                            paused: sink_ref.is_paused(),
                                            progress_info_str: format!(
                                                "{} / {}",
                                                make_time_str(_progress.min(duration)),
                                                make_time_str(duration)
                                            ),
                                        });
                                    }
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
                PlayerCommand::ChangeProgress(new_progress) => {
                    match sink.try_seek(Duration::from_secs_f32(new_progress)) {
                        Ok(_) => {
                            let mut prog = progress.lock().unwrap();
                            *prog = new_progress;
                            dbg!(*prog);
                        }
                        Err(e) => {
                            eprintln!("Failed to seek: {}", e);
                        }
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
    {
        let tx = tx.clone();
        ui.on_change_progress(move |new_progress: f32| {
            tx.send(PlayerCommand::ChangeProgress(new_progress))
                .unwrap();
        });
    }

    // UI 定时刷新进度条
    let ui_weak = ui.as_weak();
    let timer = slint::Timer::default();
    timer.start(
        slint::TimerMode::Repeated,
        Duration::from_millis(100),
        move || {
            if let Ok(state) = progress_rx.try_recv() {
                if let Some(ui) = ui_weak.upgrade() {
                    ui.set_progress(state.progress);
                    ui.set_duration(state.duration);
                    ui.set_progress_info_str(state.progress_info_str.clone().into());
                }
            }
        },
    );

    ui.run().unwrap();
}
