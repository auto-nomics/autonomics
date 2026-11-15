fn main() {
    if !manuscript_engine::run_worker_if_requested() {
        eprintln!("Internal embedded Tectonic worker; start through manuscript-engine.");
        std::process::exit(2);
    }
}
