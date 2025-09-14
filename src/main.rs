#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
use lofty::file::{AudioFile, TaggedFileExt};
use lofty::tag::Accessor;
use rand::Rng;
use rodio::{Decoder, Source};
use slint::{Model, ToSharedString};
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
    let audio_dir = home::home_dir().unwrap().join("Music");
    if !audio_dir.exists() {
        return Vec::new();
    }
    let mut list = Vec::new();
    for (index, entry) in glob::glob(audio_dir.join("*.flac").to_str().unwrap())
        .unwrap()
        .chain(glob::glob(audio_dir.join("*.mp3").to_str().unwrap()).unwrap())
        .chain(glob::glob(audio_dir.join("*.wav").to_str().unwrap()).unwrap())
        .enumerate()
    {
        if let Ok(p) = entry {
            if let Ok(tagged) = lofty::read_from_path(&p) {
                let dura = tagged.properties().duration().as_secs_f32();
                if let Some(tag) = tagged.primary_tag() {
                    let item = SongInfo {
                        id: index as i32,
                        song_path: p.display().to_shared_string(),
                        song_name: tag
                            .title()
                            .as_deref()
                            .unwrap_or(
                                p.file_stem()
                                    .map(|x| x.to_str())
                                    .flatten()
                                    .unwrap_or("unknown"),
                            )
                            .to_shared_string(),
                        singer: tag
                            .artist()
                            .as_deref()
                            .unwrap_or("unknown")
                            .to_shared_string(),
                        duration: format!("{:02}:{:02}", (dura as u32) / 60, (dura as u32) % 60)
                            .to_shared_string(),
                    };
                    list.push(item);
                }
            }
        }
    }

    list
}

fn main() {
    let ui = MainWindow::new().unwrap();
    let (tx, rx) = mpsc::channel::<PlayerCommand>();
    let mut stream_handle = rodio::OutputStreamBuilder::open_default_stream().unwrap();
    stream_handle.log_on_drop(false);
    let _sink = rodio::Sink::connect_new(&stream_handle.mixer());
    let sink = Arc::new(Mutex::new(_sink));
    let ui_state = ui.global::<UIState>();
    ui_state.set_progress(0.0);
    ui_state.set_duration(0.0);
    ui_state.set_paused(true);
    ui_state.set_play_mode(PlayMode::InOrder);
    ui_state.set_dragging(false);

    // 播放线程
    let ui_weak = ui.as_weak();
    let sink_clone = sink.clone();
    thread::spawn(move || {
        // 切换到UI线程更新歌曲列表
        let ui_weak_clone = ui_weak.clone();
        slint::invoke_from_event_loop(move || {
            if let Some(ui) = ui_weak_clone.upgrade() {
                let ui_state = ui.global::<UIState>();
                ui_state.set_song_list(read_song_list().as_slice().into());
            }
        })
        .unwrap();

        while let Ok(cmd) = rx.recv() {
            match cmd {
                PlayerCommand::Play(song_info) => {
                    let file = std::fs::File::open(&song_info.song_path).unwrap();
                    let source = Decoder::try_from(file).unwrap();
                    let dura = source
                        .total_duration()
                        .map(|d| d.as_secs_f32())
                        .unwrap_or(0.0);
                    let sink_guard = sink_clone.lock().unwrap();
                    sink_guard.stop();
                    sink_guard.clear();
                    sink_guard.append(source);
                    sink_guard.play();
                    let ui_weak = ui_weak.clone();
                    slint::invoke_from_event_loop(move || {
                        if let Some(ui) = ui_weak.upgrade() {
                            let ui_state = ui.global::<UIState>();
                            ui_state.set_current_song(song_info);
                            ui_state.set_paused(false);
                            ui_state.set_progress(0.0);
                            ui_state.set_duration(dura);
                            ui_state.set_user_listening(true);
                        }
                    })
                    .unwrap();
                }
                PlayerCommand::Pause => {
                    let sink_guard = sink_clone.lock().unwrap();
                    let ui_weak = ui_weak.clone();
                    if sink_guard.empty() {
                        // 如果当前没有播放任何歌曲，则播放第一首
                        slint::invoke_from_event_loop(move || {
                            if let Some(ui) = ui_weak.upgrade() {
                                let ui_state = ui.global::<UIState>();
                                ui.invoke_play(
                                    ui_state.get_song_list().iter().collect::<Vec<_>>()[0].clone(),
                                );
                                ui_state.set_paused(false);
                            }
                        })
                        .unwrap();
                    } else {
                        let paused = sink_guard.is_paused();
                        if paused {
                            sink_guard.play();
                        } else {
                            sink_guard.pause();
                        }
                        slint::invoke_from_event_loop(move || {
                            if let Some(ui) = ui_weak.upgrade() {
                                let ui_state = ui.global::<UIState>();
                                ui_state.set_paused(!paused);
                            }
                        })
                        .unwrap();
                    }
                }
                PlayerCommand::ChangeProgress(new_progress) => {
                    let sink_guard = sink_clone.lock().unwrap();
                    match sink_guard.try_seek(Duration::from_secs_f32(new_progress)) {
                        Ok(_) => {
                            let ui_weak = ui_weak.clone();
                            slint::invoke_from_event_loop(move || {
                                if let Some(ui) = ui_weak.upgrade() {
                                    let ui_state = ui.global::<UIState>();
                                    ui_state.set_progress(new_progress);
                                }
                            })
                            .unwrap();
                        }
                        Err(e) => {
                            eprintln!("Failed to seek: {}", e);
                        }
                    }
                }
                PlayerCommand::PlayNext => {
                    let ui_weak = ui_weak.clone();
                    slint::invoke_from_event_loop(move || {
                        if let Some(ui) = ui_weak.upgrade() {
                            let ui_state = ui.global::<UIState>();
                            let song_list: Vec<_> = ui_state.get_song_list().iter().collect();
                            let mut rng = rand::rng();
                            let next_id1 = rng.random_range(..song_list.len());
                            let id = ui_state.get_current_song().id as usize;
                            let next_id2 = if id + 1 >= song_list.len() { 0 } else { id + 1 };
                            let next_id = match ui_state.get_play_mode() {
                                PlayMode::InOrder => next_id2,
                                PlayMode::Random => next_id1,
                            };
                            if let Some(next_song) = song_list.get(next_id) {
                                let song_to_play = next_song.clone();
                                ui.invoke_play(song_to_play);
                            }
                        }
                    })
                    .unwrap();
                }
                PlayerCommand::PlayPrev => {
                    let ui_weak = ui_weak.clone();
                    slint::invoke_from_event_loop(move || {
                        if let Some(ui) = ui_weak.upgrade() {
                            let ui_state = ui.global::<UIState>();
                            let song_list: Vec<_> = ui_state.get_song_list().iter().collect();
                            let mut rng = rand::rng();
                            let next_id1 = rng.random_range(..song_list.len());
                            let id = ui_state.get_current_song().id as usize;
                            let next_id2 = if id + 1 >= song_list.len() { 0 } else { id + 1 };
                            let next_id = match ui_state.get_play_mode() {
                                PlayMode::InOrder => next_id2,
                                PlayMode::Random => next_id1,
                            };
                            if let Some(next_song) = song_list.get(next_id) {
                                let song_to_play = next_song.clone();
                                ui.invoke_play(song_to_play);
                            }
                        }
                    })
                    .unwrap();
                }
                PlayerCommand::SwitchMode(m) => {
                    let ui_weak = ui_weak.clone();
                    slint::invoke_from_event_loop(move || {
                        if let Some(ui) = ui_weak.upgrade() {
                            let ui_state = ui.global::<UIState>();
                            ui_state.set_play_mode(m);
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
    let timer = slint::Timer::default();
    let sink_clone = sink.clone();
    timer.start(
        slint::TimerMode::Repeated,
        Duration::from_millis(200),
        move || {
            let sink_guard = sink_clone.lock().unwrap();
            if let Some(ui) = ui_weak.upgrade() {
                // 如果不在拖动进度条，则自增进度条
                let ui_state = ui.global::<UIState>();
                if !ui_state.get_dragging() {
                    ui_state.set_progress(sink_guard.get_pos().as_secs_f32());
                }
                ui_state.set_progress_info_str(
                    format!(
                        "{:02}:{:02} / {:02}:{:02}",
                        (ui_state.get_progress() as u32) / 60,
                        (ui_state.get_progress() as u32) % 60,
                        (ui_state.get_duration() as u32) / 60,
                        (ui_state.get_duration() as u32) % 60
                    )
                    .to_shared_string(),
                );
                // 如果播放完毕，且之前是在播放状态，则自动播放下一首
                if sink_guard.empty() && ui_state.get_user_listening() && !ui_state.get_paused() {
                    ui.invoke_play_next();
                }
            }
        },
    );

    ui.run().unwrap();
}
