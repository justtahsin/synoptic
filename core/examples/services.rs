//! Sanity check for the systemctl-based service listing.
fn main() {
    let list = taskman_core::list_services();
    println!("{} servis bulundu; ilk 5:", list.len());
    for s in list.iter().take(5) {
        println!("  {:<40} {:<8} {:<10} {}", s.name, s.active, s.sub, s.description);
    }
}
