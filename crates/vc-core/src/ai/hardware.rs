//! What this machine has to offer the AI worker: cores, threads, memory,
//! and the thread count that makes sense for them. Probed once, cached.
//!
//! The number of inference threads used to be a constant (8). That is too
//! many for a 4-core laptop (it starves PipeWire's realtime thread and the
//! window) and too few for a 16-core desktop. Auto now derives it from the
//! topology; the user can override it, bounded by what the machine has.

use std::path::Path;
use std::sync::OnceLock;

#[derive(Debug, Clone, PartialEq)]
pub struct HardwareInfo {
    /// `model name` from /proc/cpuinfo, trimmed ("AMD Ryzen 9 5950X 16-Core Processor").
    pub cpu_model: String,
    /// Hardware threads this process may run on.
    pub logical_cpus: usize,
    /// Distinct physical cores among them.
    pub physical_cores: usize,
    /// Simultaneous multithreading is active (two threads per core).
    pub smt: bool,
    /// Mixed core types (Intel P/E cores, ARM big.LITTLE).
    pub hybrid: bool,
    /// Cores of the fastest type; equals `physical_cores` on uniform parts.
    pub performance_cores: usize,
    /// Total memory in gigabytes (0 when unknown).
    pub ram_gb: f32,
}

/// Upper bound for Auto. More intra-op threads than this buys nothing on
/// the models we run and only adds spin-wait contention.
pub const MAX_AUTO_THREADS: usize = 12;

static INFO: OnceLock<HardwareInfo> = OnceLock::new();

/// The cached probe.
pub fn info() -> &'static HardwareInfo {
    INFO.get_or_init(probe)
}

/// Read the topology from sysfs and /proc. Never fails; unknown values
/// fall back to conservative guesses.
pub fn probe() -> HardwareInfo {
    let logical = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(1);
    probe_from(
        Path::new("/sys/devices/system/cpu"),
        Path::new("/proc"),
        logical,
    )
}

fn probe_from(sys_cpu: &Path, proc_dir: &Path, logical: usize) -> HardwareInfo {
    let smt = std::fs::read_to_string(sys_cpu.join("smt/active"))
        .map(|s| s.trim() == "1")
        .unwrap_or(false);
    let online = read_cpu_list(&sys_cpu.join("online")).unwrap_or_else(|| (0..logical).collect());

    // Physical cores: unique (package, core) pairs among the online CPUs.
    let mut cores: Vec<(u32, u32)> = Vec::new();
    let mut capacities: Vec<(u32, u32, u32)> = Vec::new(); // (package, core, capacity)
    for cpu in &online {
        let topo = sys_cpu.join(format!("cpu{cpu}/topology"));
        let pkg = read_u32(&topo.join("physical_package_id")).unwrap_or(0);
        let Some(core) = read_u32(&topo.join("core_id")) else {
            continue;
        };
        if !cores.contains(&(pkg, core)) {
            cores.push((pkg, core));
            let cap = read_u32(&sys_cpu.join(format!("cpu{cpu}/cpu_capacity"))).unwrap_or(1024);
            capacities.push((pkg, core, cap));
        }
    }
    let physical_cores = if cores.is_empty() {
        if smt { (logical / 2).max(1) } else { logical }
    } else {
        cores.len().min(logical).max(1)
    };
    let max_cap = capacities.iter().map(|c| c.2).max().unwrap_or(1024);
    let hybrid = capacities.iter().any(|c| c.2 != max_cap);
    let performance_cores = if hybrid {
        capacities
            .iter()
            .filter(|c| c.2 == max_cap)
            .count()
            .clamp(1, physical_cores)
    } else {
        physical_cores
    };

    let cpuinfo = std::fs::read_to_string(proc_dir.join("cpuinfo")).unwrap_or_default();
    let cpu_model = cpuinfo
        .lines()
        .find_map(|l| {
            let (k, v) = l.split_once(':')?;
            (k.trim() == "model name" || k.trim() == "Model name" || k.trim() == "Hardware")
                .then(|| v.trim().to_string())
        })
        .unwrap_or_else(|| std::env::consts::ARCH.to_string());
    let meminfo = std::fs::read_to_string(proc_dir.join("meminfo")).unwrap_or_default();
    let ram_gb = meminfo
        .lines()
        .find_map(|l| l.strip_prefix("MemTotal:"))
        .and_then(|v| v.split_whitespace().next()?.parse::<f32>().ok())
        .map(|kb| kb / 1_048_576.0)
        .unwrap_or(0.0);

    HardwareInfo {
        cpu_model,
        logical_cpus: logical.max(1),
        physical_cores,
        smt,
        hybrid,
        performance_cores,
        ram_gb,
    }
}

fn read_u32(path: &Path) -> Option<u32> {
    std::fs::read_to_string(path).ok()?.trim().parse().ok()
}

/// Parse a sysfs CPU list such as `0-3,8,10-11`.
fn read_cpu_list(path: &Path) -> Option<Vec<usize>> {
    let text = std::fs::read_to_string(path).ok()?;
    let mut out = Vec::new();
    for part in text.trim().split(',') {
        let part = part.trim();
        if part.is_empty() {
            continue;
        }
        match part.split_once('-') {
            Some((a, b)) => {
                let (a, b): (usize, usize) = (a.parse().ok()?, b.parse().ok()?);
                out.extend(a..=b);
            }
            None => out.push(part.parse().ok()?),
        }
    }
    Some(out)
}

/// Auto policy: how many inference threads to run on this machine.
///
/// CPU inference gets the performance cores minus one (that one is kept
/// free for PipeWire's realtime thread and the window), never more than the
/// hardware threads minus one, capped at [`MAX_AUTO_THREADS`]. Two-core
/// machines get one thread. With the models on a GPU, the CPU only feeds
/// it, so a few threads are plenty.
pub fn recommended_threads(info: &HardwareInfo, gpu: bool) -> usize {
    let cores = info.performance_cores.max(1);
    let cpu_threads = if cores <= 2 {
        1
    } else {
        (cores - 1)
            .min(info.logical_cpus.saturating_sub(1).max(1))
            .min(MAX_AUTO_THREADS)
    };
    if gpu { cpu_threads.min(4) } else { cpu_threads }
}

/// The most threads the user may ask for on this machine.
pub fn max_threads() -> usize {
    info().logical_cpus.max(1)
}

/// Resolve the setting (`0` = Auto, otherwise a count) to a thread count,
/// clamped to what the machine has.
pub fn resolve_threads(setting: u32, gpu: bool) -> usize {
    let info = info();
    if setting == 0 {
        recommended_threads(info, gpu)
    } else {
        (setting as usize).clamp(1, info.logical_cpus.max(1))
    }
}

/// One line for the Settings page: "AMD Ryzen 9 5950X · 16 cores, 32 threads · 94 GB".
pub fn summary() -> String {
    let i = info();
    let mut s = format!("{} · {}", short_model(&i.cpu_model), cores_line(i));
    if i.ram_gb > 0.0 {
        s.push_str(&format!(" · {:.0} GB memory", i.ram_gb));
    }
    s
}

pub fn cores_line(i: &HardwareInfo) -> String {
    let mut s = if i.hybrid {
        format!(
            "{} cores ({} performance), {} threads",
            i.physical_cores, i.performance_cores, i.logical_cpus
        )
    } else {
        format!("{} cores, {} threads", i.physical_cores, i.logical_cpus)
    };
    if i.physical_cores == i.logical_cpus {
        s = format!("{} cores", i.physical_cores);
    }
    s
}

/// Trim marketing suffixes ("16-Core Processor", "with Radeon Graphics").
pub fn short_model(model: &str) -> String {
    let mut m = model.replace("(R)", "").replace("(TM)", "");
    if let Some(i) = m.find(" with ") {
        m.truncate(i);
    }
    if let Some(i) = m.find(" @ ") {
        m.truncate(i);
    }
    let mut m = m.split_whitespace().collect::<Vec<_>>().join(" ");
    loop {
        let before = m.len();
        for suffix in ["Processor", "CPU"] {
            if let Some(rest) = m.strip_suffix(suffix) {
                m = rest.trim_end().to_string();
            }
        }
        if let Some(i) = m.rfind("-Core")
            && m[i + 5..].trim().is_empty()
            && m[..i]
                .rsplit(' ')
                .next()
                .is_some_and(|n| n.chars().all(|c| c.is_ascii_digit()))
        {
            let cut = m[..i].rfind(' ').unwrap_or(0);
            m.truncate(cut);
            m = m.trim_end().to_string();
        }
        if m.len() == before {
            break;
        }
    }
    if m.is_empty() { model.to_string() } else { m }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hw(logical: usize, physical: usize, perf: usize) -> HardwareInfo {
        HardwareInfo {
            cpu_model: "test".into(),
            logical_cpus: logical,
            physical_cores: physical,
            smt: logical > physical,
            hybrid: perf != physical,
            performance_cores: perf,
            ram_gb: 16.0,
        }
    }

    #[test]
    fn auto_threads_follow_the_core_count() {
        // (logical, physical, performance, gpu) -> threads
        let table = [
            (32, 16, 16, false, 12), // Ryzen 5950X: capped
            (16, 8, 8, false, 7),    // 8-core desktop: one core kept free
            (12, 6, 6, false, 5),
            (8, 4, 4, false, 3), // 4-core laptop
            (4, 4, 4, false, 3),
            (4, 2, 2, false, 1), // 2-core: a single thread
            (2, 2, 2, false, 1),
            (1, 1, 1, false, 1),
            (20, 14, 6, false, 5), // Intel 12th gen: P-cores only
            (8, 8, 4, false, 3),   // big.LITTLE
            (32, 16, 16, true, 4), // GPU: a feeder only
            (4, 2, 2, true, 1),
        ];
        for (l, p, perf, gpu, want) in table {
            assert_eq!(
                recommended_threads(&hw(l, p, perf), gpu),
                want,
                "logical={l} physical={p} perf={perf} gpu={gpu}"
            );
        }
    }

    #[test]
    fn never_more_threads_than_hardware_threads() {
        for logical in 1..=64 {
            for physical in 1..=logical {
                let n = recommended_threads(&hw(logical, physical, physical), false);
                assert!(n >= 1 && n <= logical.max(1));
                assert!(n <= MAX_AUTO_THREADS);
            }
        }
    }

    #[test]
    fn override_is_clamped_to_this_machine() {
        let max = max_threads();
        assert_eq!(resolve_threads(1, false), 1);
        assert_eq!(resolve_threads(max as u32, false), max);
        assert_eq!(resolve_threads(10_000, false), max);
        assert_eq!(
            resolve_threads(0, false),
            recommended_threads(info(), false)
        );
    }

    #[test]
    fn cpu_lists_parse() {
        let dir = std::env::temp_dir().join(format!("vc-hw-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("online");
        std::fs::write(&p, "0-3,8,10-11\n").unwrap();
        assert_eq!(read_cpu_list(&p).unwrap(), vec![0, 1, 2, 3, 8, 10, 11]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn sysfs_topology_is_read() {
        let dir = std::env::temp_dir().join(format!("vc-hw-topo-{}", std::process::id()));
        let cpu = dir.join("cpu");
        let proc_ = dir.join("proc");
        std::fs::create_dir_all(cpu.join("smt")).unwrap();
        std::fs::create_dir_all(&proc_).unwrap();
        std::fs::write(cpu.join("smt/active"), "1").unwrap();
        std::fs::write(cpu.join("online"), "0-7").unwrap();
        // 4 cores x 2 threads, cores 0-1 fast, 2-3 slow.
        for c in 0..8u32 {
            let t = cpu.join(format!("cpu{c}/topology"));
            std::fs::create_dir_all(&t).unwrap();
            std::fs::write(t.join("physical_package_id"), "0").unwrap();
            std::fs::write(t.join("core_id"), (c / 2).to_string()).unwrap();
            let cap = if c < 4 { "1024" } else { "500" };
            std::fs::write(cpu.join(format!("cpu{c}/cpu_capacity")), cap).unwrap();
        }
        std::fs::write(
            proc_.join("cpuinfo"),
            "processor\t: 0\nmodel name\t: Fake Chip 4-Core Processor\n",
        )
        .unwrap();
        std::fs::write(proc_.join("meminfo"), "MemTotal:       16777216 kB\n").unwrap();
        let i = probe_from(&cpu, &proc_, 8);
        assert_eq!(i.logical_cpus, 8);
        assert_eq!(i.physical_cores, 4);
        assert!(i.smt);
        assert!(i.hybrid);
        assert_eq!(i.performance_cores, 2);
        assert_eq!(short_model(&i.cpu_model), "Fake Chip");
        assert!((i.ram_gb - 16.0).abs() < 0.01);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn real_machine_probe_is_sane() {
        let i = probe();
        assert!(i.logical_cpus >= 1);
        assert!(i.physical_cores >= 1 && i.physical_cores <= i.logical_cpus);
        assert!(i.performance_cores >= 1 && i.performance_cores <= i.physical_cores);
    }
}
