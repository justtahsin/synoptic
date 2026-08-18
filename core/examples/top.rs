//! Quick sanity check for the sampler: prints the top-5 CPU consumers.
fn main() {
    let mut sampler = synoptic_core::Sampler::new();
    let _ = sampler.sample();
    std::thread::sleep(std::time::Duration::from_millis(1000));
    let snap = sampler.sample();
    println!(
        "CPU: {:.1}%   MEM: {}/{} MB   processes: {}",
        snap.cpu_percent,
        snap.mem_used / 1048576,
        snap.mem_total / 1048576,
        snap.processes.len()
    );
    let mut procs = snap.processes;
    procs.sort_by(|a, b| b.cpu_percent.total_cmp(&a.cpu_percent));
    for p in procs.iter().take(5) {
        println!(
            "{:>7}  {:<24} {:>5.1}%  {:>9.1} MB",
            p.pid,
            p.name,
            p.cpu_percent,
            p.mem_bytes as f64 / 1048576.0
        );
    }
}
