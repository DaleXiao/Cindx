#[cfg(unix)]
unsafe extern "C" {
    fn kill(pid: i32, signal: i32) -> i32;
}

#[cfg(unix)]
pub(crate) fn terminate_process_group(process_id: u32, signal: i32) {
    unsafe {
        let _ = kill(-(process_id as i32), signal);
    }
}

#[cfg(not(unix))]
pub(crate) fn terminate_process_group(_process_id: u32, _signal: i32) {}
