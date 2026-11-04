fn main() {
    let path = std::env::args().nth(1).expect("path");
    println!(
        "{}",
        pactree::file_format::file_storage_audit_json(std::path::Path::new(&path)).expect("audit")
    );
}
