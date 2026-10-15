#[path = "../src/mongo_fixture.rs"]
mod fixture;
fn main() {
    use std::io::BufRead;
    let fixture = fixture::Fixture::start();
    println!("{}", fixture.port);
    for line in std::io::stdin().lock().lines().map_while(Result::ok) {
        if line.trim() == "quit" {
            break;
        }
        if line.trim() == "status"
            && let Ok(state) = fixture.state.lock()
        {
            println!(
                "finds={} writes={} documents={}",
                state.finds,
                state.writes,
                state.documents.len()
            );
        }
    }
}
