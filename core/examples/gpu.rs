//! Sanity check for GPU sampling.
fn main() {
    let mut sampler = synoptic_core::Sampler::new();
    let snap = sampler.sample();
    println!("{} GPU bulundu:", snap.gpus.len());
    for g in &snap.gpus {
        println!(
            "  {} | {} | yük: {:?} | VRAM: {:?}/{:?} | sıcaklık: {:?}",
            g.id, g.name, g.busy_percent, g.vram_used, g.vram_total, g.temp_c
        );
    }
}
