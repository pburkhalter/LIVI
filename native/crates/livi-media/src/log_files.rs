//! The logs of the helpers LIVI starts (compositor, gst-host): each start used to create its file anew, so a problem
//! seen before a restart had no log left once anyone could look (a black reversing camera, then the ignition off and
//! on). The file of the run before goes to `<name>.1.log`, that one to `.2`, as `LIVI.log` keeps its own. A run that
//! writes more than MAX_BYTES (a debug switch left on, an error in every frame) goes on in a fresh file the same way:
//! a helper's logs never take more than (KEEP + 1) x MAX_BYTES, as LIVI.log's do not.

use std::fs::{self, File};
use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::Stdio;

/// How many files of earlier runs stay beside the current one
const KEEP: usize = 3;
/// The most one file takes before the next one begins (LIVI.log's own, livi-core log_file.rs)
const MAX_BYTES: u64 = 8 * 1024 * 1024;

fn numbered(path: &Path, i: usize) -> PathBuf {
    if i == 0 {
        return path.to_path_buf();
    }
    let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("log");
    let ext = path.extension().and_then(|s| s.to_str()).unwrap_or("log");
    path.with_file_name(format!("{stem}.{i}.{ext}"))
}

/// A new, empty log at `path`, the earlier ones moved one place on; None when the folder cannot be made
pub(crate) fn fresh(path: &Path) -> Option<File> {
    fs::create_dir_all(path.parent()?).ok()?;
    let _ = fs::remove_file(numbered(path, KEEP));
    for i in (0..KEEP).rev() {
        let _ = fs::rename(numbered(path, i), numbered(path, i + 1));
    }
    File::create(path).ok()
}

/// stdout and stderr for a helper: one pipe whose lines a thread of LIVI's writes to a fresh log at `path`, and to a
/// fresh one again past MAX_BYTES. The thread ends with the helper (the pipe's last writer). None when the folder or
/// the pipe cannot be made
pub(crate) fn capped(path: &Path) -> Option<(Stdio, Stdio)> {
    let file = fresh(path)?;
    let (from, to) = std::io::pipe().ok()?;
    let to_err = to.try_clone().ok()?;
    let path = path.to_path_buf();
    std::thread::Builder::new()
        .name("helper-log".into())
        .spawn(move || copy(from, file, &path, MAX_BYTES))
        .ok()?;
    Some((Stdio::from(to), Stdio::from(to_err)))
}

/// Whole lines from `from` into `file`, a fresh file (`fresh`) once `file` holds more than `max` bytes
fn copy(from: impl Read, mut file: File, path: &Path, max: u64) {
    let mut lines = BufReader::new(from);
    let mut line = Vec::new();
    let mut written = 0;
    loop {
        line.clear();
        match lines.read_until(b'\n', &mut line) {
            Ok(0) | Err(_) => return,
            Ok(_) => {}
        }
        if written > max
            && let Some(next) = fresh(path)
        {
            file = next;
            written = 0;
        }
        if file.write_all(&line).is_ok() {
            written += line.len() as u64;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_dir::TempDir;
    use std::io::Write;

    fn run(path: &Path, text: &str) {
        fresh(path).unwrap().write_all(text.as_bytes()).unwrap();
    }

    #[test]
    fn the_runs_before_stay_beside_it() {
        let dir = TempDir::new();
        let log = dir.0.join("log/gst-host.log");
        for run_no in 1..=5 {
            run(&log, &format!("run {run_no}"));
        }
        let read = |p: PathBuf| fs::read_to_string(p).unwrap();
        assert_eq!(read(log.clone()), "run 5");
        assert_eq!(read(dir.0.join("log/gst-host.1.log")), "run 4");
        assert_eq!(read(dir.0.join("log/gst-host.3.log")), "run 2");
        assert!(!dir.0.join("log/gst-host.4.log").exists());
    }

    #[test]
    fn a_run_that_writes_much_goes_on_in_a_fresh_file_and_the_oldest_go() {
        let dir = TempDir::new();
        let log = dir.0.join("log/gst-host.log");
        let lines: String = (0..100).map(|i| format!("line {i:03}\n")).collect();
        // 9 bytes a line, a fresh file once one holds more than 90
        copy(lines.as_bytes(), fresh(&log).unwrap(), &log, 90);
        let read = |i: usize| fs::read_to_string(numbered(&log, i)).unwrap();
        for i in 0..=KEEP {
            assert!(read(i).len() <= 90 + 9, "file {i}");
        }
        assert!(!numbered(&log, KEEP + 1).exists());
        // the newest lines, whole and in order, the oldest gone
        let kept: String = (0..=KEEP).rev().map(read).collect();
        assert!(lines.ends_with(&kept) && kept.len() < lines.len());
        assert!(read(0).ends_with("line 099\n"));
    }

    #[tokio::test]
    async fn a_helper_s_output_goes_through_the_pipe_into_its_log() {
        let dir = TempDir::new();
        let log = dir.0.join("log/compositor.log");
        let (out, err) = capped(&log).unwrap();
        let status = tokio::process::Command::new("sh")
            .args(["-c", "echo out; echo err >&2"])
            .stdout(out)
            .stderr(err)
            .status()
            .await
            .unwrap();
        assert!(status.success());
        // the thread writes once the helper has ended
        let text = (0..100)
            .find_map(|_| {
                let text = fs::read_to_string(&log).unwrap_or_default();
                if text.lines().count() == 2 {
                    return Some(text);
                }
                std::thread::sleep(std::time::Duration::from_millis(20));
                None
            })
            .unwrap();
        assert!(text.contains("out\n") && text.contains("err\n"), "{text}");
    }

    #[test]
    fn the_first_run_makes_the_folder() {
        let dir = TempDir::new();
        let log = dir.0.join("new/compositor.log");
        assert!(fresh(&log).is_some());
        assert!(log.exists());
        assert!(!dir.0.join("new/compositor.1.log").exists());
    }
}
