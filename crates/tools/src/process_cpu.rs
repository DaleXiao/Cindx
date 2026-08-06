#[cfg(target_os = "macos")]
use std::ffi::c_void;

#[cfg(target_os = "macos")]
const RUSAGE_INFO_V2: i32 = 2;
#[cfg(target_os = "macos")]
const MAX_GROUP_PIDS: usize = 1_024;

#[cfg(target_os = "macos")]
#[repr(C)]
#[derive(Default)]
struct RusageInfoV2 {
    uuid: [u8; 16],
    user_time: u64,
    system_time: u64,
    package_idle_wakeups: u64,
    interrupt_wakeups: u64,
    pageins: u64,
    wired_size: u64,
    resident_size: u64,
    physical_footprint: u64,
    process_start_abstime: u64,
    process_exit_abstime: u64,
    child_user_time: u64,
    child_system_time: u64,
    child_package_idle_wakeups: u64,
    child_interrupt_wakeups: u64,
    child_pageins: u64,
    child_elapsed_abstime: u64,
    disk_bytes_read: u64,
    disk_bytes_written: u64,
}

#[cfg(target_os = "macos")]
#[link(name = "proc")]
unsafe extern "C" {
    fn proc_listpgrppids(group_id: i32, buffer: *mut c_void, buffer_size: i32) -> i32;
    fn proc_pid_rusage(process_id: i32, flavor: i32, buffer: *mut c_void) -> i32;
}

#[cfg(target_os = "macos")]
pub(crate) fn process_group_cpu_nanos(group_id: u32) -> Option<u64> {
    let required = unsafe { proc_listpgrppids(group_id as i32, std::ptr::null_mut(), 0) };
    if required <= 0 {
        return None;
    }
    let slots = (required as usize).clamp(1, MAX_GROUP_PIDS);
    let mut pids = vec![0i32; slots];
    let returned = unsafe {
        proc_listpgrppids(
            group_id as i32,
            pids.as_mut_ptr().cast(),
            (pids.len() * std::mem::size_of::<i32>()) as i32,
        )
    };
    if returned <= 0 {
        return None;
    }
    let count = (returned as usize).min(pids.len());
    let mut total = 0u64;
    let mut observed = false;
    for process_id in pids.into_iter().take(count).filter(|pid| *pid > 0) {
        let mut usage = RusageInfoV2::default();
        let result = unsafe {
            proc_pid_rusage(
                process_id,
                RUSAGE_INFO_V2,
                (&mut usage as *mut RusageInfoV2).cast(),
            )
        };
        if result == 0 {
            observed = true;
            total = total
                .saturating_add(usage.user_time)
                .saturating_add(usage.system_time)
                .saturating_add(usage.child_user_time)
                .saturating_add(usage.child_system_time);
        }
    }
    observed.then_some(total)
}

#[cfg(not(target_os = "macos"))]
pub(crate) fn process_group_cpu_nanos(_group_id: u32) -> Option<u64> {
    None
}
