//! The logs of the helpers LIVI starts (compositor, gst-host): each start used to create its file anew, so a problem
//! seen before a restart had no log left once anyone could look (a black reversing camera, then the ignition off and
//! on). The file of the run before goes to `<name>.1.log`, that one to `.2`, as `LIVI.log` keeps its own.

use std::fs::{self, File};
use std::path::{Path, PathBuf};

/// How many files of earlier runs stay beside the current one
const KEEP: usize = 3;

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
    fn the_first_run_makes_the_folder() {
        let dir = TempDir::new();
        let log = dir.0.join("new/compositor.log");
        assert!(fresh(&log).is_some());
        assert!(log.exists());
        assert!(!dir.0.join("new/compositor.1.log").exists());
    }
}
