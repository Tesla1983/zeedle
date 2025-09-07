#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

use rodio::{Decoder, OutputStream, Sink};
use slint::SharedString;

slint::include_modules!();

enum PlayerCommand {
    Play(String),
    Pause,
}

fn main() {
    let ui = MainWindow::new().unwrap();

    let (tx, rx) = mpsc::channel::<PlayerCommand>();

    // 播放线程
    thread::spawn(move || {
        let (_stream, handle) = OutputStream::try_default().unwrap();
        let mut sink = Sink::try_new(&handle).unwrap();

        while let Ok(cmd) = rx.recv() {
            match cmd {
                PlayerCommand::Play(path) => {
                    if let Ok(file) = std::fs::File::open(&path) {
                        if let Ok(source) = Decoder::new(std::io::BufReader::new(file)) {
                            sink.stop();
                            sink = Sink::try_new(&handle).unwrap();
                            sink.append(source);
                            sink.play();
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

    // UI 和后台通信
    let tx_play = tx.clone();
    ui.on_play(move |path: SharedString| {
        tx_play.send(PlayerCommand::Play(path.to_string())).unwrap();
    });

    let tx_pause = tx.clone();
    ui.on_pause(move || {
        tx_pause.send(PlayerCommand::Pause).unwrap();
    });

    // 定时刷新 UI 状态
    let ui_weak = ui.as_weak();
    slint::Timer::default().start(
        slint::TimerMode::Repeated,
        Duration::from_millis(500),
        move || {
            if let Some(ui) = ui_weak.upgrade() {
                ui.set_status(SharedString::from("Running..."));
            }
        },
    );

    ui.run().unwrap();
}
