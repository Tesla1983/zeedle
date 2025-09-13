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

trait Builder {
    fn progress(self, new_progress: f32) -> Self;
    fn duration(self, new_duration: f32) -> Self;
    fn paused(self, is_paused: bool) -> Self;
    fn dragging(self, is_dragging: bool) -> Self;
    fn song_list(self, list: Vec<SongInfo>) -> Self;
    fn current_song(self, song: SongInfo) -> Self;
    fn play_mode(self, mode: PlayMode) -> Self;
    fn user_listening(self, listening: bool) -> Self;
}

impl Builder for UIState {
    fn progress(mut self, new_progress: f32) -> Self {
        self.progress = new_progress;
        self.progress_info_str = format!(
            "{:02}:{:02} / {:02}:{:02}",
            self.progress as i32 / 60,
            self.progress as i32 % 60,
            self.duration as i32 / 60,
            self.duration as i32 % 60
        )
        .to_shared_string();
        self
    }
    fn duration(mut self, new_duration: f32) -> Self {
        self.duration = new_duration;
        self.progress_info_str = format!(
            "{:02}:{:02} / {:02}:{:02}",
            self.progress as i32 / 60,
            self.progress as i32 % 60,
            self.duration as i32 / 60,
            self.duration as i32 % 60
        )
        .to_shared_string();
        self
    }
    fn paused(mut self, is_paused: bool) -> Self {
        self.paused = is_paused;
        self
    }
    fn dragging(mut self, is_dragging: bool) -> Self {
        self.dragging = is_dragging;
        self
    }
    fn song_list(mut self, list: Vec<SongInfo>) -> Self {
        self.song_list = list.as_slice().into();
        self
    }
    fn current_song(mut self, song: SongInfo) -> Self {
        self.current_song = song;
        self
    }
    fn play_mode(mut self, mode: PlayMode) -> Self {
        self.play_mode = mode;
        self
    }
    fn user_listening(mut self, listening: bool) -> Self {
        self.user_listening = listening;
        self
    }
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

fn main() {
    let ui = MainWindow::new().unwrap();
    let (tx, rx) = mpsc::channel::<PlayerCommand>();
    let mut stream_handle = rodio::OutputStreamBuilder::open_default_stream().unwrap();
    stream_handle.log_on_drop(false);
    let _sink = rodio::Sink::connect_new(&stream_handle.mixer());
    let sink = Arc::new(Mutex::new(_sink));
    let init_ui_state = ui
        .get_ui_state()
        .progress(0.0)
        .duration(0.0)
        .paused(true)
        .play_mode(PlayMode::InOrder)
        .dragging(false);
    ui.set_ui_state(init_ui_state);

    // 播放线程
    let ui_weak = ui.as_weak();
    let sink_clone = sink.clone();
    thread::spawn(move || {
        // 切换到UI线程更新歌曲列表
        let ui_weak_clone = ui_weak.clone();
        slint::invoke_from_event_loop(move || {
            if let Some(ui) = ui_weak_clone.upgrade() {
                ui.set_ui_state(ui.get_ui_state().song_list(read_song_list()));
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
                            let new_ui_state = ui
                                .get_ui_state()
                                .current_song(song_info)
                                .paused(false)
                                .progress(0.)
                                .duration(dura)
                                .user_listening(true);
                            ui.set_ui_state(new_ui_state);
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
                                let ui_state = ui.get_ui_state();
                                ui.invoke_play(
                                    ui_state.song_list.iter().collect::<Vec<_>>()[0].clone(),
                                );
                                ui.set_ui_state(ui_state.paused(false));
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
                                let ui_state = ui.get_ui_state();
                                ui.set_ui_state(ui_state.paused(!paused));
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
                                    let new_ui_state = ui.get_ui_state().progress(new_progress);
                                    ui.set_ui_state(new_ui_state);
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
                            let ui_state = ui.get_ui_state();
                            let song_list: Vec<_> = ui_state.song_list.iter().collect();
                            let mut rng = rand::rng();
                            let next_id1 = rng.random_range(..song_list.len());
                            let id = ui_state.current_song.id as usize;
                            let next_id2 = if id + 1 >= song_list.len() { 0 } else { id + 1 };
                            let next_id = match ui_state.play_mode {
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
                            let ui_state = ui.get_ui_state();
                            let song_list: Vec<_> = ui_state.song_list.iter().collect();
                            let mut rng = rand::rng();
                            let next_id1 = rng.random_range(..song_list.len());
                            let id = ui_state.current_song.id as usize;
                            let next_id2 = if id + 1 >= song_list.len() { 0 } else { id + 1 };
                            let next_id = match ui_state.play_mode {
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
                            let new_ui_state = ui.get_ui_state().play_mode(m);
                            ui.set_ui_state(new_ui_state);
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
                let ui_state = ui.get_ui_state();
                if !ui_state.dragging {
                    ui.set_ui_state(
                        ui.get_ui_state()
                            .progress(sink_guard.get_pos().as_secs_f32()),
                    );
                }
                // 如果播放完毕，且之前是在播放状态，则自动播放下一首
                if sink_guard.empty() && ui_state.user_listening && !ui_state.paused {
                    ui.invoke_play_next();
                }
            }
        },
    );

    ui.run().unwrap();
}
