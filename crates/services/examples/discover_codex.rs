fn main() {
    let candidates = turbodbn_services::codex_discovery::discover();
    for candidate in &candidates {
        println!(
            "{} | {} | {}",
            candidate.source,
            candidate.version,
            candidate.path.display()
        );
    }
    println!("{} verified candidates", candidates.len());
}
