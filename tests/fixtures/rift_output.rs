use std::io::Write;

fn main() {
    let mode = std::fs::read_to_string("output-mode.txt").unwrap();
    std::fs::write("wrapper.marker", "ran").unwrap();
    if mode == "missing" {
        std::process::exit(2);
    }
    if mode != "cache" {
        std::fs::write("tgstation.dmb", b"rift fixture dmb").unwrap();
        std::fs::write("tgstation.rsc", b"rift fixture rsc").unwrap();
    }
    match mode.as_str() {
        "error" => println!("fixture.dm:7: error: early build failure"),
        "malformed" | "duplicate" => println!("RIFT_RESULT {{invalid}}"),
        "cache" => println!("Skipping 'dm' (up to date)"),
        "many" => {
            for line in 1..=30000 {
                println!("fixture.dm:{line}: error: failure {line}");
            }
        }
        "oversized" => println!("{}", "x".repeat(1024 * 1024 + 100)),
        _ => {}
    }
    if !matches!(mode.as_str(), "quiet" | "many" | "oversized") {
        let noise = "ordinary build progress\n".repeat(30000);
        std::io::stdout().write_all(noise.as_bytes()).unwrap();
        if mode == "flood" {
            std::io::stderr().write_all(noise.as_bytes()).unwrap();
        }
    }
    // Only duplicate needs a later valid record to hide the evicted malformed one.
    // Ordinary fresh-artifact and exact cache-marker compatibility remain exercised.
    if mode == "duplicate" {
        println!("RIFT_RESULT {{\"schema_version\":1,\"run_id\":\"20260906T120000Z-0123abcd\",\"command\":\"compile\",\"status\":\"passed\",\"evidence\":\"full_build\",\"exit_code\":0,\"reused\":false,\"artifacts\":[{{\"path\":\"artifacts/tgstation.dmb\",\"size\":16,\"sha256\":\"20f614d7c24b63b1f46e89b6a095a4223f6d2115d7aa9fa3cc96f732d57a05a7\",\"freshness\":\"rebuilt\"}},{{\"path\":\"artifacts/tgstation.rsc\",\"size\":16,\"sha256\":\"31ad65cec09cb7d48945fbedbf19d83c7c38165f1b5474cba7b2b4510ffc461c\",\"freshness\":\"rebuilt\"}}]}}");
    }
}
