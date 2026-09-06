use std::io::Write;
fn main() {
    let arguments: Vec<_> = std::env::args().skip(1).collect();
    let dme = std::path::Path::new(arguments.last().unwrap());
    std::fs::write(dme.with_extension("started"), "started").unwrap();
    std::fs::write(dme.with_extension("dmb"), "owned output fixture").unwrap();
    let mode = arguments
        .iter()
        .find_map(|arg| arg.strip_prefix("-DOUTPUT_MODE="))
        .unwrap_or("quiet");
    match mode {
        "dual" | "moderate" => {
            let bytes = vec![b'x'; if mode == "dual" { 600_000 } else { 40_000 }];
            std::io::stdout().write_all(&bytes).unwrap();
            std::io::stderr().write_all(&bytes).unwrap();
        }
        "unicode" => print!("{}", "🛰\u{1}\n".repeat(90_000)),
        "controls" => {
            let bytes: Vec<_> = (0..=31).cycle().take(100_000).collect();
            std::io::stdout().write_all(&bytes).unwrap();
            std::io::stderr().write_all(&bytes).unwrap();
        }
        "diagnostics" | "few_diagnostics" => {
            for line in 1..=if mode == "diagnostics" { 600 } else { 20 } {
                println!("fixture.dm:{line}: error: failed check {line}");
            }
            for line in 1..=10 {
                eprintln!("fixture.dm:{line}: warning: caution {line}");
            }
        }
        "giant" => {
            println!("fixture.dm:7: error: {}", "\u{1}🛰".repeat(20_000));
            println!("fixture.dm:8: warning: later warning");
        }
        _ => println!("compiler note"),
    }
}
