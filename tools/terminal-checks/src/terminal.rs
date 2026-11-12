use std::{
    fs,
    path::Path,
    process::Command,
    thread,
    time::{Duration, Instant},
};
use terminal_pilot_rust::{
    pty::{Options, Pty},
    session::Session,
    strip_ansi,
};
use terminal_png_rust::{ansi, png, raster, svg};

pub struct Terminal {
    pty: Pty,
    screen: Session,
    pending: Vec<u8>,
    start: Instant,
}
impl Terminal {
    pub fn open(cmd: Command, cols: usize, rows: usize) -> Self {
        let pty = Pty::spawn(Options {
            command: cmd.get_program().to_str().unwrap().into(),
            args: cmd.get_args().map(|s| s.to_str().unwrap().into()).collect(),
            cwd: cmd.get_current_dir().map(|p| p.to_str().unwrap().into()),
            env: cmd
                .get_envs()
                .filter_map(|(k, v)| Some((k.to_str()?.into(), v?.to_str()?.into())))
                .collect(),
            cols,
            rows,
        })
        .unwrap();
        Self {
            pty,
            screen: Session::new(vec![], cols as f64, rows as f64, 0.).unwrap(),
            pending: vec![],
            start: Instant::now(),
        }
    }
    fn pump(&mut self) {
        loop {
            let part = self.pty.read().unwrap();
            if part.is_empty() {
                break;
            }
            self.pending.extend(part);
            let n = match std::str::from_utf8(&self.pending) {
                Ok(s) => s.len(),
                Err(e) if e.error_len().is_none() => e.valid_up_to(),
                Err(e) => panic!("invalid terminal UTF-8: {e}"),
            };
            let chars = std::str::from_utf8(&self.pending[..n])
                .unwrap()
                .encode_utf16()
                .collect::<Vec<_>>();
            self.screen
                .data(&chars, self.start.elapsed().as_secs_f64() * 1000.)
                .unwrap();
            self.pending.drain(..n);
        }
    }
    pub fn text(&self) -> String {
        String::from_utf16_lossy(&strip_ansi(&self.screen.screen_lines().join(&10)))
    }
    pub fn history(&self) -> String {
        String::from_utf16_lossy(&self.screen.history(None).unwrap().join(&10))
    }
    pub fn wait(&mut self, needle: &str) {
        let end = Instant::now() + Duration::from_secs(12);
        loop {
            self.pump();
            if self.text().contains(needle) {
                return;
            }
            assert!(
                Instant::now() < end && self.pty.exit_code().unwrap().is_none(),
                "missing {needle:?}:\n{}",
                self.text()
            );
            thread::sleep(Duration::from_millis(15))
        }
    }
    pub fn key(&mut self, s: &str) {
        self.pty.write(s.as_bytes()).unwrap()
    }
    pub fn resize(&mut self, w: usize, h: usize) {
        self.pty.resize(w, h).unwrap();
        self.screen.resize(w as f64, h as f64).unwrap()
    }
    pub fn exit(&mut self) -> i32 {
        let end = Instant::now() + Duration::from_secs(12);
        loop {
            self.pump();
            if let Some(code) = self.pty.exit_code().unwrap() {
                self.pump();
                return code;
            }
            assert!(Instant::now() < end, "did not exit:\n{}", self.text());
            thread::sleep(Duration::from_millis(15))
        }
    }
    pub fn save(&mut self, path: &Path) {
        let end = Instant::now() + Duration::from_secs(5);
        loop {
            self.pump();
            if self
                .screen
                .quiet_remaining(100., self.start.elapsed().as_secs_f64() * 1000.)
                .unwrap()
                == 0.
            {
                break;
            }
            assert!(Instant::now() < end, "screen did not settle");
            thread::sleep(Duration::from_millis(15))
        }
        let raw = self.screen.screen_lines().join(&10);
        let img =
            raster::render(&svg::render(&ansi::parse(&raw), svg::Options::default())).unwrap();
        fs::write(path, png::encode(img.width, img.height, &img.rgba).unwrap()).unwrap();
    }
}
