use std::sync::{Arc, Mutex, mpsc};
use std::thread;
use std::time::Duration;

use rodio::{Decoder, OutputStream, Sink, Source};
use slint::{SharedString, ToSharedString};

slint::include_modules!();

// ui --> backend
enum PlayerCommand {
    Play(String),        // 从头播放某个音频文件
    Pause,               // 暂停/继续播放
    ChangeProgress(f32), // 拖拽进度条
}

fn read_song_list() -> Vec<SongInfo> {
    let mut list = Vec::new();
    for entry in glob::glob("./audios/*.mp3")
        .unwrap()
        .chain(glob::glob("./audios/*.flac").unwrap())
    {
        if let Ok(path) = entry {
            let tag = audiotags::Tag::new().read_from_path(&path).unwrap();
            let file = std::fs::File::open(&path).unwrap();
            let source = Decoder::new(std::io::BufReader::new(file)).unwrap();
            let dura = source
                .total_duration()
                .map(|d| d.as_secs_f32())
                .unwrap_or(0.0);

            list.push(SongInfo {
                song_name: tag
                    .title()
                    .unwrap_or(path.file_stem().map(|x| x.to_str()).unwrap().unwrap())
                    .to_shared_string(),
                singer: tag.artist().unwrap_or("unknown").to_shared_string(),
                duration: format!("{:02}:{:02}", (dura as u32) / 60, (dura as u32) % 60)
                    .to_shared_string(),
                song_path: path.display().to_shared_string(),
            });
        }
    }

    list
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
    let (_stream, handle) = OutputStream::try_default().unwrap();
    let sink = Arc::new(Mutex::new(Sink::try_new(&handle).unwrap()));
    let progress = Arc::new(Mutex::new(0.0f32));
    let duration = Arc::new(Mutex::new(0.0f32));

    // 播放线程
    let ui_weak = ui.as_weak();
    let sink_clone = sink.clone();
    let _prog = progress.clone();
    let _dura = duration.clone();
    thread::spawn(move || {
        let song_list = read_song_list();
        // 切换到UI线程更新歌曲列表
        let ui_weak_clone = ui_weak.clone();
        slint::invoke_from_event_loop(move || {
            if let Some(ui) = ui_weak_clone.upgrade() {
                ui.set_song_list(song_list.as_slice().into());
            }
        })
        .unwrap();

        while let Ok(cmd) = rx.recv() {
            match cmd {
                PlayerCommand::Play(path) => {
                    let file = std::fs::File::open(&path).unwrap();
                    let source = Decoder::new(std::io::BufReader::new(file)).unwrap();

                    *_prog.lock().unwrap() = 0.0;
                    *_dura.lock().unwrap() = source
                        .total_duration()
                        .map(|d| d.as_secs_f32())
                        .unwrap_or(0.0);
                    let mut _sink = sink_clone.lock().unwrap();
                    _sink.stop();
                    *_sink = Sink::try_new(&handle).unwrap();
                    _sink.append(source);
                    _sink.play();

                    // 切换到主线程更新UI
                    let ui_weak = ui_weak.clone();
                    let _dura = _dura.clone();
                    slint::invoke_from_event_loop(move || {
                        if let Some(ui) = ui_weak.upgrade() {
                            ui.set_duration(_dura.lock().unwrap().clone());
                            ui.set_progress(0.0);
                            ui.set_progress_info_str("00:00".into());
                            ui.set_paused(false);
                        }
                    })
                    .unwrap();
                }
                PlayerCommand::Pause => {
                    let _sink = sink_clone.lock().unwrap();
                    if _sink.is_paused() {
                        _sink.play();
                    } else {
                        _sink.pause();
                    }
                }
                PlayerCommand::ChangeProgress(new_progress) => {
                    let _sink = sink_clone.lock().unwrap();
                    match _sink.try_seek(Duration::from_secs_f32(new_progress)) {
                        Ok(_) => {
                            *_prog.lock().unwrap() = new_progress;
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
        ui.on_toggle_play(move || {
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
    let _prog = progress.clone();
    let _dura = duration.clone();
    let timer = slint::Timer::default();
    let sink_clone = sink.clone();
    timer.start(
        slint::TimerMode::Repeated,
        Duration::from_millis(200),
        move || {
            let _sink = sink_clone.lock().unwrap();
            if let Some(ui) = ui_weak.upgrade() {
                // 如果不在拖动进度条，则更新进度条
                if !ui.get_dragging() {
                    ui.set_progress(_sink.get_pos().as_secs_f32());
                }
                ui.set_duration(_dura.lock().unwrap().clone());
                ui.set_progress_info_str(
                    format!(
                        "{:02}/{:02}",
                        make_time_str(_sink.get_pos().as_secs_f32()),
                        make_time_str(_dura.lock().unwrap().clone())
                    )
                    .into(),
                );
            }
        },
    );

    ui.run().unwrap();
}
