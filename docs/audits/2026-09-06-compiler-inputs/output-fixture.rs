use std::io::Write;
fn main() {
    let arguments: Vec<_> = std::env::args_os().skip(1).collect();
    let dme = std::path::Path::new(arguments.last().unwrap());
    std::fs::write(dme.with_extension("dmb"), b"owned compiler output fixture").unwrap();
    let bytes = vec![b'x'; 600_000];
    std::io::stdout().write_all(&bytes).unwrap();
    std::io::stderr().write_all(&bytes).unwrap();
}
