use std::{
    fs,
    io::{self, Write},
    path::{Path, PathBuf},
};

use env_logger::Target;
use log::LevelFilter;

struct MultiWriter {
    console: Box<dyn Write + Send>,
    file: Box<dyn Write + Send>,
}

impl Write for MultiWriter {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.console.write(buf)?;
        self.file.write(buf)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.console.flush()?;
        self.file.flush()
    }
}

fn get_log_path() -> PathBuf {
    let f_name = ".zeedle.log";
    if let Some(mut p) = home::home_dir() {
        p.push(f_name);
        p
    } else {
        PathBuf::from(f_name)
    }
}

pub fn init_default_logger(path: Option<impl AsRef<Path>>) {
    let log_path = if let Some(p) = path {
        p.as_ref().to_path_buf()
    } else {
        get_log_path()
    };
    if log_path.exists() {
        if fs::metadata(&log_path).unwrap().len() > 1024 * 1024 * 10 {
            fs::remove_file(&log_path).expect("Failed to remove old log file");
        }
    }
    let log_file = fs::OpenOptions::new()
        .create(true)
        .write(true)
        .append(true)
        .open(&log_path)
        .expect("can't open this file!");
    let log_target = Box::new(MultiWriter {
        console: Box::new(io::stdout()),
        file: Box::new(log_file),
    });
    env_logger::builder()
        .format(move |buf, record| {
            writeln!(
                buf,
                "[{} | {} | {}:{}] --> {}",
                chrono::Local::now().format("%Y-%m-%d %H:%M:%S"),
                record.level(),
                record.file().unwrap_or("unknown"),
                record.line().unwrap_or(0),
                record.args()
            )
        })
        .filter(None, LevelFilter::Info) // 设置日志级别为Info
        .target(Target::Pipe(log_target))
        .init();
}
