//! Sanity check for XDG autostart listing.
fn main() {
    let list = synoptic_core::list_startup();
    println!("{} başlangıç girdisi:", list.len());
    for e in &list {
        println!(
            "  [{}] {:<30} {:<10} {}",
            if e.enabled { "x" } else { " " },
            e.name,
            if e.user_level {
                "kullanıcı"
            } else {
                "sistem"
            },
            e.exec
        );
    }
}
