fn main() {
    println!("Hello World from PID 1!");

    // PID 1 darf niemals beenden, sonst kernel panic
    loop {
        std::thread::sleep(std::time::Duration::from_secs(1));
    }
}
