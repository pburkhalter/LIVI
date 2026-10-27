pub fn local_now() -> libc::tm {
    local_at(unsafe { libc::time(std::ptr::null_mut()) })
}

/// Local time of a Unix time in seconds
pub fn local_at(secs: libc::time_t) -> libc::tm {
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    unsafe { libc::localtime_r(&secs, &mut tm) };
    tm
}

pub fn now_text() -> String {
    let tm = local_now();
    let sep = if tm.tm_sec % 2 == 0 { ':' } else { ' ' };
    format!("{:02}{sep}{:02}", tm.tm_hour, tm.tm_min)
}

pub fn stamp() -> String {
    let mut tv: libc::timeval = libc::timeval {
        tv_sec: 0,
        tv_usec: 0,
    };
    unsafe { libc::gettimeofday(&mut tv, std::ptr::null_mut()) };
    let now = tv.tv_sec;
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    unsafe { libc::localtime_r(&now, &mut tm) };
    format!(
        "{:02}:{:02}:{:02}.{:03}",
        tm.tm_hour,
        tm.tm_min,
        tm.tm_sec,
        tv.tv_usec / 1000
    )
}
