use std::io::Write;
use std::path::PathBuf;

fn main() {
    let args: Vec<_> = std::env::args_os().collect();
    let value =
        |name: &str| PathBuf::from(&args[args.iter().position(|arg| arg == name).unwrap() + 1]);
    let dme = value("-e");
    let project = dme.parent().unwrap();
    let output = value("--output");
    let mode = std::fs::read_to_string(project.join("mode.txt")).unwrap();
    std::fs::write(project.join("helper.pid"), std::process::id().to_string()).unwrap();
    std::fs::write(
        output.join("index.html"),
        "<html>owned documentation</html>",
    )
    .unwrap();
    std::fs::create_dir(output.join("types")).unwrap();
    std::fs::write(output.join("types/example.html"), "<html>example</html>").unwrap();
    match mode.as_str() {
        "new_input" => {
            std::fs::write(project.join("SpacemanDMM.toml"), "[dmdoc]\nmodule_directories = [\"manual\"]\n").unwrap();
            std::fs::write(project.join("manual/late.md"), "preserve late source").unwrap();
        }
        "collision" => {
            let destination = output.parent().unwrap().join("html");
            std::fs::create_dir(&destination).unwrap();
            std::fs::write(destination.join("sentinel.txt"), "preserve collision").unwrap();
        }
        "fail" => {
            eprintln!("owned helper failed");
            std::process::exit(2);
        }
        "missing_index" => std::fs::remove_file(output.join("index.html")).unwrap(),
        "wait" => {
            let mut options = std::fs::OpenOptions::new();
            options.read(true).write(true);
            #[cfg(windows)]
            {
                use std::os::windows::fs::OpenOptionsExt;
                options.share_mode(0);
            }
            let _held = options.open(output.join("index.html")).unwrap();
            std::fs::write(project.join("ready"), "ready").unwrap();
            loop {
                std::fs::write(
                    project.join("heartbeat"),
                    format!("{:?}", std::time::SystemTime::now()),
                )
                .unwrap();
                std::thread::sleep(std::time::Duration::from_millis(25));
            }
        }
        "flood" | "flood_fail" => {
            let text = "\u{1}🛰\n".repeat(150000);
            std::io::stdout().write_all(text.as_bytes()).unwrap();
            std::io::stderr().write_all(text.as_bytes()).unwrap();
            if mode == "flood_fail" {
                std::process::exit(2);
            }
        }
        _ => {}
    }
}
