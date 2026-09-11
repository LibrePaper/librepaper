//! Bounded, optional host measurements. Unsupported hosts report null fields.
use serde_json::{json, Value};
use std::io::Read;
use std::path::Path;

fn read(path: &str) -> String {
    let mut text = String::new();
    if let Ok(file) = std::fs::File::open(path) {
        let _ = file.take(64 * 1024).read_to_string(&mut text);
    }
    text
}

fn number(text: &str, field: &str) -> Option<u64> {
    text.lines().find_map(|line| {
        let (name, rest) = line.split_once(':')?;
        (name == field)
            .then(|| rest.split_whitespace().next()?.parse().ok())
            .flatten()
    })
}

pub fn snapshot() -> Value {
    let status = read("/proc/self/status");
    let memory = read("/proc/meminfo");
    let io = read("/proc/self/io");
    let scheduler = read("/proc/self/schedstat");
    let process = read("/proc/self/stat");
    let cgroup_limit = read("/sys/fs/cgroup/memory.max").trim().parse::<u64>().ok();
    let cgroup_used = read("/sys/fs/cgroup/memory.current")
        .trim()
        .parse::<u64>()
        .ok();
    // The command name may contain spaces and parentheses; fields after its
    // last closing parenthesis start with the process state (field 3).
    let fields: Vec<_> = process
        .rsplit_once(')')
        .map(|(_, rest)| rest.split_whitespace().collect())
        .unwrap_or_default();
    let ticks = |index| {
        fields
            .get(index)
            .and_then(|value: &&str| value.parse::<u64>().ok())
    };
    json!({
        "rss_bytes":number(&status,"VmRSS").map(|n|n.saturating_mul(1024)),
        "peak_rss_bytes":number(&status,"VmHWM").map(|n|n.saturating_mul(1024)),
        "host_memory_bytes":number(&memory,"MemTotal").map(|n|n.saturating_mul(1024)),
        "host_available_memory_bytes":number(&memory,"MemAvailable").map(|n|n.saturating_mul(1024)),
        "cgroup_memory_limit_bytes":cgroup_limit,
        "cgroup_memory_used_bytes":cgroup_used,
        "cpu_user_clock_ticks":ticks(11),
        "cpu_system_clock_ticks":ticks(12),
        "main_thread_runtime_nanoseconds":scheduler.split_whitespace().next().and_then(|n|n.parse::<u64>().ok()),
        "disk_read_bytes":number(&io,"read_bytes"),
        "disk_write_bytes":number(&io,"write_bytes")
    })
}

pub fn warn(config: &crate::config::Configuration, primary: &Path) {
    if let Ok(free) = fs2::available_space(primary) {
        if config.storage.total >= 0
            && config.storage.total as u64 > free.saturating_sub(256 * 1024 * 1024)
        {
            eprintln!("warning: the storage ceiling leaves less than 256 MiB of currently available filesystem headroom");
        }
    }
    let measurements = snapshot();
    let memory = ["host_memory_bytes", "cgroup_memory_limit_bytes"]
        .iter()
        .filter_map(|name| measurements[*name].as_u64())
        .min();
    if let Some(memory) = memory {
        let rooms = config.session.rooms_bytes_max as u64;
        let staging = config.persistence().max_staging_bytes as u64;
        let incoming = config.cost.request_body_memory_bytes as u64;
        if rooms.saturating_add(staging).saturating_add(incoming) > memory.saturating_mul(3) / 4 {
            eprintln!("warning: room, persistence and request-body memory ceilings exceed three quarters of detected host/container memory; leave space for the binary, proxy and operating system");
        }
    }
}
