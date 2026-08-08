#[cfg(target_os = "macos")]
use std::sync::OnceLock;

#[cfg(target_os = "macos")]
#[repr(C)]
#[derive(Default)]
struct MachTimebaseInfo {
    numer: u32,
    denom: u32,
}

#[cfg(target_os = "macos")]
unsafe extern "C" {
    fn mach_timebase_info(info: *mut MachTimebaseInfo) -> i32;
}

#[cfg(target_os = "macos")]
pub(crate) fn absolute_time_to_nanos(value: u64) -> Option<u64> {
    static TIMEBASE: OnceLock<Option<(u64, u64)>> = OnceLock::new();
    let (numerator, denominator) = (*TIMEBASE.get_or_init(|| {
        let mut timebase = MachTimebaseInfo::default();
        let result = unsafe { mach_timebase_info(&mut timebase) };
        (result == 0 && timebase.denom != 0)
            .then_some((u64::from(timebase.numer), u64::from(timebase.denom)))
    }))?;
    scale_absolute_time_to_nanos(value, numerator, denominator)
}

#[cfg(target_os = "macos")]
fn scale_absolute_time_to_nanos(value: u64, numerator: u64, denominator: u64) -> Option<u64> {
    if denominator == 0 {
        return None;
    }
    let nanos = u128::from(value)
        .saturating_mul(u128::from(numerator))
        .checked_div(u128::from(denominator))?;
    Some(nanos.min(u128::from(u64::MAX)) as u64)
}

#[cfg(all(test, target_os = "macos"))]
mod tests {
    use super::scale_absolute_time_to_nanos;

    #[test]
    fn mach_absolute_time_is_scaled_to_nanoseconds_without_overflow() {
        assert_eq!(
            scale_absolute_time_to_nanos(24_000_000, 125, 3),
            Some(1_000_000_000)
        );
        assert_eq!(
            scale_absolute_time_to_nanos(u64::MAX, u32::MAX.into(), 1),
            Some(u64::MAX)
        );
        assert_eq!(scale_absolute_time_to_nanos(1, 1, 0), None);
    }
}
