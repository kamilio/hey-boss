use std::{
    collections::VecDeque,
    fs::{self, File, OpenOptions},
    io::{self, BufRead, Write},
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};
use tracing_subscriber::{Layer, layer::SubscriberExt, util::SubscriberInitExt};

const MAX_BYTES: u64 = 2 * 1024 * 1024;
const ARCHIVES: usize = 4;

pub fn directory(port: u16) -> PathBuf {
    dirs::cache_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("hey-gh/logs")
        .join(port.to_string())
}

pub fn init(directory: Option<&Path>) -> io::Result<()> {
    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| "hey_gh=info".into());
    let stderr = tracing_subscriber::fmt::layer().with_writer(std::io::stderr);
    if let Some(directory) = directory {
        let writer = RotatingWriter::new(directory, MAX_BYTES)?;
        let file = tracing_subscriber::fmt::layer()
            .with_ansi(false)
            .with_writer(move || writer.clone())
            // Dependency debug output can include URLs and payloads. Persist
            // only the application's deliberately selected diagnostic fields.
            .with_filter(tracing_subscriber::filter::filter_fn(|metadata| {
                metadata.target().starts_with("hey_gh")
            }));
        tracing_subscriber::registry()
            .with(filter)
            .with(stderr)
            .with(file)
            .init();
    } else {
        tracing_subscriber::registry()
            .with(filter)
            .with(stderr)
            .init();
    }
    Ok(())
}

#[derive(Clone)]
struct RotatingWriter(Arc<Mutex<LogFile>>);

struct LogFile {
    directory: PathBuf,
    file: Option<File>,
    size: u64,
    limit: u64,
    _lock: File,
}

fn open_private(path: &Path) -> io::Result<File> {
    let mut options = OpenOptions::new();
    options.create(true).append(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let file = options.open(path)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        file.set_permissions(fs::Permissions::from_mode(0o600))?;
    }
    Ok(file)
}

impl RotatingWriter {
    fn new(directory: &Path, limit: u64) -> io::Result<Self> {
        fs::create_dir_all(directory)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(directory, fs::Permissions::from_mode(0o700))?;
        }
        let lock = open_private(&directory.join("writer.lock"))?;
        lock.try_lock().map_err(|_| {
            io::Error::other(
                "another hey-gh daemon owns this log directory; use a different --log-dir",
            )
        })?;
        let file = open_private(&directory.join("hey-gh.log"))?;
        let size = file.metadata()?.len();
        Ok(Self(Arc::new(Mutex::new(LogFile {
            directory: directory.to_owned(),
            file: Some(file),
            size,
            limit,
            _lock: lock,
        }))))
    }
}

impl Write for RotatingWriter {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        let mut state = self
            .0
            .lock()
            .map_err(|_| io::Error::other("log writer poisoned"))?;
        if bytes.is_empty() {
            return Ok(0);
        }
        if state.size > 0 && state.size.saturating_add(bytes.len() as u64) > state.limit {
            drop(state.file.take());
            for archive in (1..=ARCHIVES).rev() {
                let path = state.directory.join(format!("hey-gh.log.{archive}"));
                if archive == ARCHIVES {
                    match fs::remove_file(&path) {
                        Ok(()) => {}
                        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                        Err(error) => return Err(error),
                    }
                } else if path.exists() {
                    fs::rename(
                        path,
                        state.directory.join(format!("hey-gh.log.{}", archive + 1)),
                    )?;
                }
            }
            fs::rename(
                state.directory.join("hey-gh.log"),
                state.directory.join("hey-gh.log.1"),
            )?;
            state.size = 0;
        }
        if state.file.is_none() {
            state.file = Some(open_private(&state.directory.join("hey-gh.log"))?);
        }
        // Keep an individual event bounded as well as the overall file set.
        let written = bytes.len().min(state.limit as usize);
        state
            .file
            .as_mut()
            .expect("opened log file")
            .write_all(&bytes[..written])?;
        state.size += written as u64;
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        let mut state = self
            .0
            .lock()
            .map_err(|_| io::Error::other("log writer poisoned"))?;
        if let Some(file) = &mut state.file {
            file.flush()?;
        }
        Ok(())
    }
}

pub fn tail(directory: &Path, count: usize) -> io::Result<()> {
    if !(1..=10_000).contains(&count) {
        return Err(io::Error::other("--tail must be 1..10000"));
    }
    let lines = read_tail(directory, count)?;
    if lines.is_empty() {
        eprintln!("No daemon logs yet in {}", directory.display());
    }
    let stdout = io::stdout();
    let mut output = stdout.lock();
    for line in lines {
        writeln!(output, "{line}")?;
    }
    Ok(())
}

fn read_tail(directory: &Path, count: usize) -> io::Result<VecDeque<String>> {
    let mut lines = VecDeque::new();
    for archive in (0..=ARCHIVES).rev() {
        let name = if archive == 0 {
            "hey-gh.log".to_owned()
        } else {
            format!("hey-gh.log.{archive}")
        };
        let file = match File::open(directory.join(name)) {
            Ok(file) => file,
            Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
            Err(error) => return Err(error),
        };
        for line in io::BufReader::new(file).lines() {
            lines.push_back(line?);
            if lines.len() > count {
                lines.pop_front();
            }
        }
    }
    Ok(lines)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rotation_is_bounded_and_tail_survives_restart() {
        let root = tempfile::tempdir().unwrap();
        let mut writer = RotatingWriter::new(root.path(), 4).unwrap();
        for number in 0..8 {
            writeln!(writer, "{number}").unwrap();
        }
        drop(writer);
        let mut writer = RotatingWriter::new(root.path(), 4).unwrap();
        writeln!(writer, "8").unwrap();
        writeln!(writer, "9").unwrap();
        let tail: Vec<_> = read_tail(root.path(), 3).unwrap().into_iter().collect();
        assert_eq!(tail, ["7", "8", "9"]);
        assert_eq!(
            fs::read_dir(root.path())
                .unwrap()
                .filter(|entry| entry
                    .as_ref()
                    .unwrap()
                    .file_name()
                    .to_string_lossy()
                    .starts_with("hey-gh.log"))
                .count(),
            5
        );
        assert!(
            fs::read_dir(root.path())
                .unwrap()
                .filter_map(Result::ok)
                .filter(|entry| entry
                    .file_name()
                    .to_string_lossy()
                    .starts_with("hey-gh.log"))
                .all(|entry| entry.metadata().unwrap().len() <= 4)
        );
    }

    #[test]
    fn log_directory_has_a_single_private_writer() {
        let root = tempfile::tempdir().unwrap();
        let writer = RotatingWriter::new(root.path(), 100).unwrap();
        assert!(RotatingWriter::new(root.path(), 100).is_err());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                fs::metadata(root.path()).unwrap().permissions().mode() & 0o777,
                0o700
            );
            assert_eq!(
                fs::metadata(root.path().join("hey-gh.log"))
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o777,
                0o600
            );
        }
        drop(writer);
        assert!(RotatingWriter::new(root.path(), 100).is_ok());
    }
}
