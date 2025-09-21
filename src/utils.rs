use std::{path::PathBuf};

use globset::GlobBuilder;
use lofty::{
    file::{AudioFile, TaggedFileExt},
    picture::PictureType,
    tag::{Accessor, ItemKey},
};
use rayon::{
    iter::{IndexedParallelIterator, IntoParallelIterator, ParallelIterator},
    slice::ParallelSliceMut,
};
use slint::{SharedString, ToSharedString};
use walkdir::WalkDir;

use crate::slint_types::{LyricItem, SongInfo};

/// Scan songs in Path `p` and return a list of SongInfo
pub fn read_song_list(p: PathBuf) -> Vec<SongInfo> {
    let audio_dir = p.clone();
    if !audio_dir.exists() {
        return Vec::new();
    }
    let glober = GlobBuilder::new("**/*.{mp3,flac,wav,ogg}")
        .build()
        .unwrap()
        .compile_matcher();
    let entries = WalkDir::new(audio_dir)
        .into_iter()
        .filter_map(|x| x.ok())
        .filter(|x| glober.is_match(x.path()))
        .collect::<Vec<_>>();
    let max_length = 17;
    let mut songs = entries
        .into_par_iter()
        .map(|entry| {
            if let Ok(tagged) = lofty::read_from_path(&entry.path()) {
                let dura = tagged.properties().duration().as_secs_f32();
                if let Some(tag) = tagged.primary_tag() {
                    let item = SongInfo {
                        id: 0,
                        song_path: entry.path().display().to_shared_string(),
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
                    return Some(item);
                }
            }
            None
        })
        .flatten()
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
        .collect::<Vec<_>>();
    songs.par_sort_by_key(|x| x.song_name.clone());
    songs
        .into_par_iter()
        .enumerate()
        .map(|(idx, mut x)| {
            x.id = idx as i32;
            x
        })
        .collect::<Vec<_>>()
}

/// Read lyrics from audio file `p`, return a list of LyricItem
pub fn read_lyrics(p: PathBuf) -> Vec<LyricItem> {
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

/// Read album cover from audio file `p`, return a slint::Image
pub fn read_album_cover(p: PathBuf) -> Option<(Vec<u8>, u32, u32)> {
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
                    return Some((buffer, width, height));
                }
            }
        }
    }
    None
}

pub fn from_image_to_slint(buffer: Vec<u8>, width: u32, height: u32) -> slint::Image {
    let mut pixel_buffer = slint::SharedPixelBuffer::new(width, height);
    let pixel_buffer_data = pixel_buffer.make_mut_bytes();
    pixel_buffer_data.copy_from_slice(&buffer);
    return slint::Image::from_rgba8(pixel_buffer);
}

pub fn get_default_album_cover() -> slint::Image {
    slint::Image::load_from_svg_data(include_bytes!("../ui/cover.svg")).unwrap()
}

/// Get about info string
pub fn get_about_info() -> SharedString {
    format!(
        "{}\n{}\nAuthor: {}\nVersion: {}",
        env!("CARGO_PKG_NAME"),
        env!("CARGO_PKG_DESCRIPTION"),
        env!("CARGO_PKG_AUTHORS"),
        env!("CARGO_PKG_VERSION")
    )
    .into()
}
