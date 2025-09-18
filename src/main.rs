#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
use rand::Rng;
use rodio::{Decoder, Source, cpal};
use slint::{Model, ToSharedString};
use std::path::PathBuf;
use std::sync::{Arc, Mutex, mpsc};
use std::thread;
use std::time::Duration;
mod slint_types;
use slint_types::*;
mod config;
use config::Config;
mod utils;

/// Message in channel: ui --> backend
/// Note: messages in the opposite direction (backend --> ui) are sent via slint::invoke_from_event_loop
enum PlayerCommand {
    Play(SongInfo, TriggerSource), // 从头播放某个音频文件
    Pause,                         // 暂停/继续播放
    ChangeProgress(f32),           // 拖拽进度条
    PlayNext,                      // 播放下一首
    PlayPrev,                      // 播放上一首
    SwitchMode(PlayMode),          // 切换播放模式
    RefreshSongList(PathBuf),      // 刷新歌曲列表
}

/// Set UI state to default (no song)
fn set_raw_ui_state(ui: &MainWindow) {
    let ui_state = ui.global::<UIState>();
    ui_state.set_progress(0.0);
    ui_state.set_duration(0.0);
    ui_state.set_about_info(utils::get_about_info());
    ui_state.set_album_image(
        slint::Image::load_from_svg_data(include_bytes!("../ui/cover.svg")).unwrap(),
    );
    ui_state.set_current_song(SongInfo {
        id: -1,
        song_path: "".into(),
        song_name: "No song".into(),
        singer: "unknown".into(),
        duration: "00:00".into(),
    });
    ui_state.set_lyrics(Vec::new().as_slice().into());
    ui_state.set_progress_info_str("00:00 / 00:00".to_shared_string());
    ui_state.set_song_list(Vec::new().as_slice().into());
    ui_state.set_song_dir(Config::default().song_dir.to_str().unwrap().into());
    ui_state.set_play_mode(PlayMode::InOrder);
    ui_state.set_paused(true);
    ui_state.set_dragging(false);
    ui_state.set_user_listening(false);
    ui_state.set_lyric_viewport_y(0.);
}

/// Set UI state according to saved config
fn set_start_ui_state(ui: &MainWindow, sink: &rodio::Sink) {
    let ui_state = ui.global::<UIState>();
    let cfg = Config::load();
    let song_list = utils::read_song_list(cfg.song_dir.clone());
    ui_state.set_progress(cfg.progress);
    ui_state.set_duration(cfg.duration);
    ui_state.set_paused(true);
    ui_state.set_play_mode(cfg.play_mode);
    ui_state.set_song_list(song_list.as_slice().into());
    ui_state.set_song_dir(cfg.song_dir.to_str().unwrap().into());
    ui_state.set_about_info(utils::get_about_info());

    if let Some(song_info) = song_list.get(cfg.current_song_id.unwrap_or(0)) {
        ui_state.set_current_song(song_info.clone());
        ui_state.set_lyrics(
            utils::read_lyrics(song_info.song_path.as_str().into())
                .as_slice()
                .into(),
        );
        ui_state.set_album_image(utils::read_album_cover(song_info.song_path.as_str().into()));
        let file = std::fs::File::open(&song_info.song_path).unwrap();
        let source = Decoder::try_from(file).unwrap();
        sink.append(source);
        sink.pause();
        sink.try_seek(Duration::from_secs_f32(cfg.progress))
            .expect("failed to seek");
    }
}

fn main() {
    let ins = single_instance::SingleInstance::new("Vanilla Player").unwrap();
    if !ins.is_single() {
        println!("程序已经在运行！");
        return;
    }
    let mut stream_handle = rodio::OutputStreamBuilder::from_default_device()
        .expect("no output device available")
        .with_buffer_size(cpal::BufferSize::Fixed(4096))
        .open_stream()
        .expect("failed to open output stream");
    stream_handle.log_on_drop(false);
    let _sink = rodio::Sink::connect_new(&stream_handle.mixer());
    let sink = Arc::new(Mutex::new(_sink));
    // 创建消息通道 ui --> backend
    let (tx, rx) = mpsc::channel::<PlayerCommand>();
    // 初始化 UI 状态
    let ui = MainWindow::new().unwrap();
    set_start_ui_state(&ui, &sink.lock().unwrap());

    // 播放线程
    let ui_weak = ui.as_weak();
    let sink_clone = sink.clone();
    thread::spawn(move || {
        while let Ok(cmd) = rx.recv() {
            match cmd {
                PlayerCommand::Play(song_info, trigger) => {
                    let file = std::fs::File::open(&song_info.song_path).unwrap();
                    let source = Decoder::try_from(file).unwrap();
                    let lyrics = utils::read_lyrics(song_info.song_path.as_str().into());
                    let dura = source
                        .total_duration()
                        .map(|d| d.as_secs_f32())
                        .unwrap_or(0.0);
                    let sink_guard = sink_clone.lock().unwrap();
                    sink_guard.clear();
                    sink_guard.append(source);
                    sink_guard.play();
                    let ui_weak = ui_weak.clone();
                    slint::invoke_from_event_loop(move || {
                        if let Some(ui) = ui_weak.upgrade() {
                            let ui_state = ui.global::<UIState>();

                            match trigger {
                                TriggerSource::ClickItem => {
                                    let mut history =
                                        ui_state.get_play_history().iter().collect::<Vec<_>>();
                                    history.push(song_info.clone());
                                    ui_state.set_play_history(history.as_slice().into());
                                    ui_state.set_history_index(0);
                                }
                                TriggerSource::Prev => {
                                    let history =
                                        ui_state.get_play_history().iter().collect::<Vec<_>>();
                                    let new_index = ui_state.get_history_index() + 1;
                                    ui_state
                                        .set_history_index(new_index.min(history.len() as i32 - 1));
                                }
                                TriggerSource::Next => {
                                    if ui_state.get_history_index() > 0 {
                                        ui_state
                                            .set_history_index(ui_state.get_history_index() - 1);
                                    } else {
                                        let mut history =
                                            ui_state.get_play_history().iter().collect::<Vec<_>>();
                                        history.push(song_info.clone());
                                        ui_state.set_play_history(history.as_slice().into());
                                        ui_state.set_history_index(0);
                                    }
                                }
                            }

                            ui_state.set_current_song(song_info.clone());
                            ui_state.set_paused(false);
                            ui_state.set_progress(0.0);
                            ui_state.set_duration(dura);
                            ui_state.set_user_listening(true);
                            ui_state.set_lyrics(lyrics.as_slice().into());
                            ui_state.set_lyric_viewport_y(0.);
                            ui_state.set_album_image(utils::read_album_cover(
                                song_info.song_path.as_str().into(),
                            ));

                            println!(
                                "{:?} / {}",
                                ui_state
                                    .get_play_history()
                                    .iter()
                                    .map(|x| x.id)
                                    .collect::<Vec<_>>(),
                                ui_state.get_history_index()
                            );
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
                                if let Some(song) = ui_state.get_song_list().iter().next() {
                                    ui.invoke_play(song.clone(), TriggerSource::ClickItem);
                                    ui_state.set_paused(false);
                                }
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
                            // 如果处在历史播放模式，则先尝试从历史记录中获取下一首
                            if ui_state.get_history_index() > 0 {
                                let history =
                                    ui_state.get_play_history().iter().collect::<Vec<_>>();
                                if let Some(song) = history
                                    .iter()
                                    .rev()
                                    .nth((ui_state.get_history_index() - 1) as usize)
                                {
                                    ui.invoke_play(song.clone(), TriggerSource::Next);
                                }
                            } else {
                                // 否则根据播放模式获取下一首
                                let song_list: Vec<_> = ui_state.get_song_list().iter().collect();
                                if !song_list.is_empty() {
                                    let mut rng = rand::rng();
                                    let next_id1 = rng.random_range(..song_list.len());
                                    let id = ui_state.get_current_song().id as usize;
                                    let mut next_id2 =
                                        if id + 1 >= song_list.len() { 0 } else { id + 1 };
                                    next_id2 = next_id2.min(song_list.len() - 1);
                                    let next_id = match ui_state.get_play_mode() {
                                        PlayMode::InOrder => next_id2,
                                        PlayMode::Random => next_id1,
                                        PlayMode::Recursive => id,
                                    };
                                    if let Some(next_song) = song_list.get(next_id) {
                                        let song_to_play = next_song.clone();
                                        ui.invoke_play(song_to_play.clone(), TriggerSource::Next);
                                    }
                                }
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
                            let cur_song = ui_state.get_current_song();
                            let history = ui_state.get_play_history().iter().collect::<Vec<_>>();
                            if let Some(song) = history
                                .iter()
                                .rev()
                                .nth((ui_state.get_history_index() + 1) as usize)
                            {
                                ui.invoke_play(song.clone(), TriggerSource::Prev);
                            } else {
                                ui.invoke_play(cur_song, TriggerSource::Prev);
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
                PlayerCommand::RefreshSongList(path) => {
                    let new_list = utils::read_song_list(path.clone());
                    let ui_weak = ui_weak.clone();
                    let sink_clone = sink_clone.clone();
                    slint::invoke_from_event_loop(move || {
                        if let Some(ui) = ui_weak.upgrade() {
                            let ui_state = ui.global::<UIState>();
                            ui_state.set_song_list(new_list.as_slice().into());
                            if let Some(first_song) = new_list.first() {
                                ui.invoke_play(first_song.clone(), TriggerSource::ClickItem);
                            } else {
                                let sink_guard = sink_clone.lock().unwrap();
                                sink_guard.clear();
                                set_raw_ui_state(&ui);
                            }
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
        ui.on_play(move |song_info: SongInfo, trigger: TriggerSource| {
            tx.send(PlayerCommand::Play(song_info, trigger)).unwrap();
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
    {
        let tx = tx.clone();
        ui.on_refresh_song_list(move |path| {
            tx.send(PlayerCommand::RefreshSongList(path.as_str().into()))
                .unwrap();
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
                if !ui_state.get_paused() {
                    for (idx, item) in ui_state.get_lyrics().iter().enumerate() {
                        if (item.time - ui_state.get_progress()).abs() < 0.10 {
                            if idx <= 5 {
                                ui_state.set_lyric_viewport_y(0.)
                            } else {
                                ui_state.set_lyric_viewport_y(
                                    (5 as f32 - idx as f32) * ui_state.get_lyric_line_height(),
                                );
                            }
                            break;
                        }
                    }
                }
                // 如果播放完毕，且之前是在播放状态，则自动播放下一首
                if sink_guard.empty() && ui_state.get_user_listening() && !ui_state.get_paused() {
                    ui.invoke_play_next();
                }
            }
        },
    );

    // 显示 UI
    ui.run().unwrap();

    // 退出前保存状态
    let ui_state = ui.global::<UIState>();
    Config::save({
        Config {
            song_dir: ui_state.get_song_dir().as_str().into(),
            current_song_id: Some(ui_state.get_current_song().id as usize),
            progress: ui_state.get_progress(),
            duration: ui_state.get_duration(),
            play_mode: ui_state.get_play_mode(),
        }
    });
}
