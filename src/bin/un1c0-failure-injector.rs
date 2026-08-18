use std::env;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::Path;
use std::process;

fn main() {
    let mut args = env::args().skip(1);
    let mode = args.next().unwrap_or_default();
    let target = args.next().unwrap_or_default();
    if mode != "snapshot-power-loss" || target.is_empty() {
        eprintln!("usage: un1c0-failure-injector snapshot-power-loss <snapshot-path>");
        process::exit(2);
    }
    let target = Path::new(&target);
    let parent = target.parent().unwrap_or_else(|| Path::new("."));
    fs::create_dir_all(parent).expect("create snapshot directory");
    let temporary = parent.join(format!(
        ".{}.tmp",
        target.file_name().and_then(|name| name.to_str()).unwrap_or("snapshot")
    ));
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(temporary)
        .expect("create staging file");
    file.write_all(b"{\"partial\":true")
        .and_then(|_| file.sync_all())
        .expect("stage partial snapshot");
    process::abort();
}
