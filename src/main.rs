#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
use lofty::file::{AudioFile, TaggedFileExt};
use lofty::picture::PictureType;
use lofty::tag::{Accessor, ItemKey};
use rand::Rng;
use rodio::{Decoder, Source, cpal};
use slint::{Model, ToSharedString};
use std::path::PathBuf;
use std::sync::{Arc, Mutex, mpsc};
use std::thread;
use std::time::Duration;
slint::include_modules!();

#[derive(Clone, Debug, serde::Deserialize, serde::Serialize)]
struct Config {
    song_dir: PathBuf,
    current_song_id: Option<usize>,
    progress: f32,
    duration: f32,
    play_mode: PlayMode,
}
impl Default for Config {
    fn default() -> Self {
        Self {
            song_dir: home::home_dir()
                .expect("no home directory found")
                .join("Music"),
            current_song_id: None,
            progress: 0.0,
            duration: 0.0,
            play_mode: PlayMode::InOrder,
        }
    }
}

impl Config {
    fn load() -> Self {
        let cfg_path = get_cfg_path();
        if cfg_path.exists() {
            let content = std::fs::read_to_string(&cfg_path).expect("failed to read config file");
            toml::from_str(&content).unwrap_or_default()
        } else {
            Self::default()
        }
    }

    fn save(self) {
        let cfg_path = get_cfg_path();
        if let Some(parent) = cfg_path.parent() {
            std::fs::create_dir_all(parent).expect("failed to create config directory");
        }
        let content = toml::to_string_pretty(&self).expect("failed to serialize config");
        std::fs::write(cfg_path, content).expect("failed to write config file");
    }
}

fn get_cfg_path() -> PathBuf {
    home::home_dir()
        .expect("no home directory found")
        .join(".config/vanilla-player/config.toml")
}

// ui --> backend
enum PlayerCommand {
    Play(SongInfo),           // 从头播放某个音频文件
    Pause,                    // 暂停/继续播放
    ChangeProgress(f32),      // 拖拽进度条
    PlayNext,                 // 播放下一首
    PlayPrev,                 // 播放上一首
    SwitchMode(PlayMode),     // 切换播放模式
    RefreshSongList(PathBuf), // 刷新歌曲列表
}

fn read_song_list(p: PathBuf) -> Vec<SongInfo> {
    let audio_dir = p.clone();
    if !audio_dir.exists() {
        return Vec::new();
    }
    let mut list = Vec::new();
    for entry in glob::glob(audio_dir.join("*.flac").to_str().unwrap())
        .unwrap()
        .chain(glob::glob(audio_dir.join("*.mp3").to_str().unwrap()).unwrap())
        .chain(glob::glob(audio_dir.join("*.wav").to_str().unwrap()).unwrap())
    {
        if let Ok(p) = entry {
            if let Ok(tagged) = lofty::read_from_path(&p) {
                let dura = tagged.properties().duration().as_secs_f32();
                if let Some(tag) = tagged.primary_tag() {
                    let item = SongInfo {
                        id: list.len() as i32,
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

    let max_length = 17;
    list.into_iter()
        .map(|mut x| {
            let singer_chars = x.singer.chars().collect::<Vec<char>>();
            if singer_chars.len() > max_length {
                x.singer = format!(
                    "{}...",
                    singer_chars[..max_length].iter().collect::<String>()
                )
                .into();
            }
            let song_name_chars = x.song_name.chars().collect::<Vec<char>>();
            if song_name_chars.len() > max_length {
                x.song_name = format!(
                    "{}...",
                    song_name_chars[..max_length].iter().collect::<String>()
                )
                .into();
            }
            return x;
        })
        .collect()
}

fn read_lyrics(p: PathBuf) -> Vec<LyricItem> {
    if let Ok(tagged) = lofty::read_from_path(&p) {
        if let Some(tag) = tagged.primary_tag() {
            if let Some(lyric_item) = tag.get(&ItemKey::Lyrics) {
                let mut lyrics = lyric_item
                    .value()
                    .text()
                    .unwrap()
                    .split("\n")
                    .map(|line| {
                        let (time_str, text) = line.split_once(']').unwrap_or(("", ""));
                        let time_str = time_str.trim_start_matches('[');
                        let dura = time_str
                            .split(':')
                            .map(|x| x.parse::<f32>().unwrap_or(0.))
                            .rev()
                            .reduce(|acc, x| acc + x * 60.)
                            .unwrap_or(0.);
                        LyricItem {
                            time: dura,
                            text: text.to_shared_string(),
                            duration: 0.0,
                        }
                    })
                    .filter(|ins| ins.time > 0. && !ins.text.is_empty())
                    .collect::<Vec<_>>();
                for i in 0..lyrics.len() - 1 {
                    lyrics[i].duration = lyrics[i + 1].time - lyrics[i].time;
                }
                lyrics.last_mut().map(|ins| ins.duration = 100.0);
                return lyrics;
            }
        }
    }
    return Vec::new();
}

fn read_album_cover(p: PathBuf) -> slint::Image {
    if let Ok(tagged) = lofty::read_from_path(&p) {
        if let Some(tag) = tagged.primary_tag() {
            if let Some(picture) = tag.pictures().iter().find(|pic| {
                pic.pic_type() == PictureType::CoverFront
                    || pic.pic_type() == PictureType::CoverBack
            }) {
                if let Ok(img) = image::load_from_memory(picture.data()) {
                    let rgba = img.into_rgba8();
                    let (width, height) = rgba.dimensions();
                    let buffer = rgba.into_vec();
                    let mut pixel_buffer = slint::SharedPixelBuffer::new(width, height);
                    let pixel_buffer_data = pixel_buffer.make_mut_bytes();
                    pixel_buffer_data.copy_from_slice(&buffer);
                    return slint::Image::from_rgba8(pixel_buffer);
                }
            }
        }
    }
    slint::Image::load_from_svg_data(include_bytes!("../ui/cover.svg")).unwrap()
}

fn main() {
    let cfg = Config::load();
    let ui = MainWindow::new().unwrap();
    let (tx, rx) = mpsc::channel::<PlayerCommand>();
    let mut stream_handle = rodio::OutputStreamBuilder::from_default_device()
        .expect("no output device available")
        .with_buffer_size(cpal::BufferSize::Fixed(4096))
        .open_stream()
        .expect("failed to open output stream");
    stream_handle.log_on_drop(false);
    let _sink = rodio::Sink::connect_new(&stream_handle.mixer());
    let sink = Arc::new(Mutex::new(_sink));
    let ui_state = ui.global::<UIState>();
    let song_list = read_song_list(cfg.song_dir.clone());
    ui_state.set_progress(cfg.progress);
    ui_state.set_duration(cfg.duration);
    ui_state.set_play_mode(cfg.play_mode);
    ui_state.set_paused(true);
    ui_state.set_dragging(false);
    ui_state.set_song_list(song_list.as_slice().into());
    ui_state.set_song_dir(cfg.song_dir.to_str().unwrap().into());
    ui_state.set_about_info(
        format!(
            "{}\n\n{}\n\nauthor: {}\n\nversion: {}",
            env!("CARGO_PKG_NAME"),
            env!("CARGO_PKG_DESCRIPTION"),
            env!("CARGO_PKG_AUTHORS"),
            env!("CARGO_PKG_VERSION")
        )
        .into(),
    );
    if let Some(song_info) = song_list.get(cfg.current_song_id.unwrap_or(0)) {
        ui_state.set_current_song(song_info.clone());
        ui_state.set_lyrics(
            read_lyrics(song_info.song_path.as_str().into())
                .as_slice()
                .into(),
        );
        ui_state.set_album_image(read_album_cover(song_info.song_path.as_str().into()));
        let file = std::fs::File::open(&song_info.song_path).unwrap();
        let source = Decoder::try_from(file).unwrap();
        let sink_guard = sink.lock().unwrap();
        sink_guard.append(source);
        sink_guard.pause();
        let _ = sink_guard.try_seek(Duration::from_secs_f32(cfg.progress));
    }

    // 播放线程
    let ui_weak = ui.as_weak();
    let sink_clone = sink.clone();
    thread::spawn(move || {
        while let Ok(cmd) = rx.recv() {
            match cmd {
                PlayerCommand::Play(song_info) => {
                    let file = std::fs::File::open(&song_info.song_path).unwrap();
                    let source = Decoder::try_from(file).unwrap();
                    let lyrics = read_lyrics(song_info.song_path.as_str().into());
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
                            ui_state.set_current_song(song_info.clone());
                            ui_state.set_paused(false);
                            ui_state.set_progress(0.0);
                            ui_state.set_duration(dura);
                            ui_state.set_user_listening(true);
                            ui_state.set_lyrics(lyrics.as_slice().into());
                            ui_state.set_lyric_viewport_y(0.);
                            ui_state.set_album_image(read_album_cover(
                                song_info.song_path.as_str().into(),
                            ));
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
                                    ui.invoke_play(song.clone());
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
                                };
                                if let Some(next_song) = song_list.get(next_id) {
                                    let song_to_play = next_song.clone();
                                    ui.invoke_play(song_to_play);
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
                            let song_list: Vec<_> = ui_state.get_song_list().iter().collect();
                            if !song_list.is_empty() {
                                let mut rng = rand::rng();
                                let next_id1 = rng.random_range(..song_list.len());
                                let id = ui_state.get_current_song().id as usize;
                                let mut next_id2 = if id as i32 - 1 < 0 { 0 } else { id - 1 };
                                next_id2 = next_id2.min(song_list.len() - 1);
                                let next_id = match ui_state.get_play_mode() {
                                    PlayMode::InOrder => next_id2,
                                    PlayMode::Random => next_id1,
                                };
                                if let Some(next_song) = song_list.get(next_id) {
                                    let song_to_play = next_song.clone();
                                    ui.invoke_play(song_to_play);
                                }
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
                    let new_list = read_song_list(path.clone());
                    let ui_weak = ui_weak.clone();
                    let sink_clone = sink_clone.clone();
                    slint::invoke_from_event_loop(move || {
                        if let Some(ui) = ui_weak.upgrade() {
                            let ui_state = ui.global::<UIState>();
                            ui_state.set_song_list(new_list.as_slice().into());
                            if let Some(first_song) = new_list.first() {
                                ui.invoke_play(first_song.clone());
                            } else {
                                let sink_guard = sink_clone.lock().unwrap();
                                sink_guard.clear();
                                ui_state.set_current_song(SongInfo {
                                    id: -1,
                                    song_path: "".into(),
                                    song_name: "No song".into(),
                                    singer: "unknown".into(),
                                    duration: "00:00".into(),
                                });
                                ui_state.set_progress(0.);
                                ui_state.set_duration(0.);
                                ui_state.set_lyrics(Vec::new().as_slice().into());
                                ui_state.set_progress_info_str("00:00 / 00:00".to_shared_string());
                                ui_state.set_album_image(
                                    slint::Image::load_from_svg_data(include_bytes!(
                                        "../ui/cover.svg"
                                    ))
                                    .unwrap(),
                                );
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
