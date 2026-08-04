use std::env;
use std::fs;
use std::thread;
use std::time::Duration;

fn main() {
    let arguments: Vec<_> = env::args().skip(1).collect();
    if arguments.len() == 2 && arguments[0] == "--sentinel" {
        thread::sleep(Duration::from_secs(7));
        fs::write(&arguments[1], b"descendant survived\n").unwrap();
        return;
    }
    if arguments == ["-n", "--version"] {
        println!("******");
        println!("** ngspice-45.2 : Circuit level simulation program");
        println!("******");
        return;
    }
    if arguments != ["-n", "-b", "input.spice"] {
        eprintln!("hang fake received unexpected argv");
        std::process::exit(2);
    }
    loop {
        thread::sleep(Duration::from_secs(60));
    }
}
