//! Пиковая память процесса для замеров (MVP-009, MVP-038).

/// Пиковое потребление памяти процессом, байт.
/// macOS: `ri_lifetime_max_phys_footprint` — то же, что «peak memory footprint» в `/usr/bin/time -l`
/// (на Apple Silicon память Metal общая, поэтому ориентируемся на footprint, BENCHMARK_PLAN §5.2).
/// Windows: не реализовано (MVP-005 и далее), возвращается `None`.
#[cfg(target_os = "macos")]
pub fn peak_memory_bytes() -> Option<u64> {
    // SAFETY: `proc_pid_rusage` заполняет переданную структуру нужного размера для текущего процесса.
    unsafe {
        let mut info: libc::rusage_info_v4 = std::mem::zeroed();
        let rc = libc::proc_pid_rusage(
            libc::getpid(),
            libc::RUSAGE_INFO_V4,
            std::ptr::addr_of_mut!(info).cast::<libc::rusage_info_t>(),
        );
        (rc == 0).then_some(info.ri_lifetime_max_phys_footprint)
    }
}

#[cfg(not(target_os = "macos"))]
pub fn peak_memory_bytes() -> Option<u64> {
    None
}
