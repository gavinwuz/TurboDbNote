fn main() {
    if let Err(message) = turbodbn_services::mysql_legacy::worker_main() {
        println!("{}", serde_json::json!({"error":message}));
        std::process::exit(1);
    }
}
