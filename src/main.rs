use rand::Rng;
use rodio::{Decoder, Source};
use slint::ToSharedString;
use std::sync::{Arc, Mutex, mpsc};
use std::thread;
use std::time::Duration;

slint::include_modules!();

// ui --> backend
enum PlayerCommand {
    Play(SongInfo),       // 从头播放某个音频文件
    Pause,                // 暂停/继续播放
    ChangeProgress(f32),  // 拖拽进度条
    PlayNext,             // 播放下一首
    PlayPrev,             // 播放上一首
    SwitchMode(PlayMode), // 切换播放模式
}

fn read_song_list() -> Vec<SongInfo> {
    let mut list = Vec::new();
    for (index, entry) in glob::glob("./audios/*.mp3")
        .unwrap()
        .chain(glob::glob("./audios/*.flac").unwrap())
        .enumerate()
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
                id: index as i32,
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
    let stream_handle = rodio::OutputStreamBuilder::open_default_stream().unwrap();
    let _sink = rodio::Sink::connect_new(&stream_handle.mixer());
    let sink = Arc::new(Mutex::new(_sink));
    let progress = Arc::new(Mutex::new(0.0f32));
    let duration = Arc::new(Mutex::new(0.0f32));
    let listening = Arc::new(Mutex::new(false));

    // 播放线程
    let ui_weak = ui.as_weak();
    let sink_clone = sink.clone();
    let _prog = progress.clone();
    let _dura = duration.clone();
    let _listening = listening.clone();
    thread::spawn(move || {
        let mut rng = rand::rng();
        let mut play_mode = PlayMode::InOrder;
        if let Some(ui) = ui_weak.upgrade() {
            play_mode = ui.get_play_mode();
        }
        let song_list = read_song_list();
        let song_list = Arc::new(song_list);
        let mut current_song = song_list.get(0).unwrap().clone();
        // 切换到UI线程更新歌曲列表
        let ui_weak_clone = ui_weak.clone();
        let song_list_clone = song_list.clone();
        slint::invoke_from_event_loop(move || {
            if let Some(ui) = ui_weak_clone.upgrade() {
                ui.set_song_list(song_list_clone.as_slice().into());
            }
        })
        .unwrap();

        while let Ok(cmd) = rx.recv() {
            match cmd {
                PlayerCommand::Play(song_info) => {
                    *_listening.lock().unwrap() = true;
                    current_song = song_info.clone();
                    let file = std::fs::File::open(&song_info.song_path).unwrap();
                    let source = Decoder::new(std::io::BufReader::new(file)).unwrap();
                    *_prog.lock().unwrap() = 0.0;
                    *_dura.lock().unwrap() = source
                        .total_duration()
                        .map(|d| d.as_secs_f32())
                        .unwrap_or(0.0);
                    let mut _sink = sink_clone.lock().unwrap();
                    _sink.stop();
                    _sink.clear();
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
                            ui.set_current_song(song_info);
                        }
                    })
                    .unwrap();
                }
                PlayerCommand::Pause => {
                    let _sink = sink_clone.lock().unwrap();
                    if _sink.empty() {
                        // 如果当前没有播放任何歌曲，则播放第一首
                        let first_song = song_list.get(0).unwrap().clone();
                        let ui_weak = ui_weak.clone();
                        slint::invoke_from_event_loop(move || {
                            if let Some(ui) = ui_weak.upgrade() {
                                ui.invoke_play(first_song);
                            }
                        })
                        .unwrap();
                    } else {
                        if _sink.is_paused() {
                            _sink.play();
                        } else {
                            _sink.pause();
                        }
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
                PlayerCommand::PlayNext => {
                    let id = current_song.id as usize;
                    let next_id = match play_mode {
                        PlayMode::InOrder => {
                            if id + 1 >= song_list.len() {
                                0
                            } else {
                                id + 1
                            }
                        }
                        PlayMode::Random => rng.random_range(..song_list.len()),
                    };
                    if let Some(next_song) = song_list.get(next_id) {
                        let ui_weak = ui_weak.clone();
                        let song_to_play = next_song.clone();
                        slint::invoke_from_event_loop(move || {
                            if let Some(ui) = ui_weak.upgrade() {
                                ui.invoke_play(song_to_play);
                            }
                        })
                        .unwrap();
                    }
                }
                PlayerCommand::PlayPrev => {
                    let id = current_song.id as usize;
                    let prev_id = if id == 0 { song_list.len() - 1 } else { id - 1 };
                    if let Some(prev_song) = song_list.get(prev_id) {
                        let ui_weak = ui_weak.clone();
                        let song_to_play = prev_song.clone();
                        slint::invoke_from_event_loop(move || {
                            if let Some(ui) = ui_weak.upgrade() {
                                ui.invoke_play(song_to_play);
                            }
                        })
                        .unwrap();
                    }
                }
                PlayerCommand::SwitchMode(m) => {
                    play_mode = m;
                    let ui_weak = ui_weak.clone();
                    slint::invoke_from_event_loop(move || {
                        if let Some(ui) = ui_weak.upgrade() {
                            ui.set_play_mode(m);
                        }
                    })
                    .unwrap();
                }
            }
        }
    });

    // UI 触发事件
    {
        let tx = tx.clone();
        ui.on_play(move |song_info: SongInfo| {
            tx.send(PlayerCommand::Play(song_info)).unwrap();
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
    {
        let tx = tx.clone();
        ui.on_play_next(move || {
            tx.send(PlayerCommand::PlayNext).unwrap();
        });
    }
    {
        let tx = tx.clone();
        ui.on_play_prev(move || {
            tx.send(PlayerCommand::PlayPrev).unwrap();
        });
    }
    {
        let tx = tx.clone();
        ui.on_switch_mode(move |play_mode| {
            tx.send(PlayerCommand::SwitchMode(play_mode)).unwrap();
        });
    }

    // UI 定时刷新进度条
    let ui_weak = ui.as_weak();
    let _prog = progress.clone();
    let _dura = duration.clone();
    let timer = slint::Timer::default();
    let sink_clone = sink.clone();
    let listening_clone = listening.clone();
    timer.start(
        slint::TimerMode::Repeated,
        Duration::from_millis(200),
        move || {
            let _sink = sink_clone.lock().unwrap();
            if let Some(ui) = ui_weak.upgrade() {
                // 如果不在拖动进度条，则自增进度条
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
                if _sink.empty() && *listening_clone.lock().unwrap() {
                    ui.invoke_play_next();
                }
            }
        },
    );

    ui.run().unwrap();
}
