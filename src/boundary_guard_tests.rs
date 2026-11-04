use std::fs;
use std::path::{Path, PathBuf};

fn src_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("src")
}

fn rust_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            rust_files(&path, out);
        } else if path.extension().is_some_and(|ext| ext == "rs") {
            out.push(path);
        }
    }
}

fn is_gated_production(path: &Path) -> bool {
    let p = path.to_string_lossy().to_lowercase();
    !p.contains("lexpack") && !p.contains("test")
}

fn result_error_types(text: &str) -> Vec<String> {
    let bytes = text.as_bytes();
    let mut out = Vec::new();
    let needle = b"Result<";
    let mut i = 0;
    while i + needle.len() <= bytes.len() {
        if &bytes[i..i + needle.len()] == needle {
            let mut depth = 0usize;
            let mut j = i + needle.len() - 1;
            let mut end = None;
            while j < bytes.len() {
                match bytes[j] {
                    b'<' => depth += 1,
                    b'>' => {
                        depth -= 1;
                        if depth == 0 {
                            end = Some(j);
                            break;
                        }
                    }
                    _ => {}
                }
                j += 1;
            }
            if let Some(end) = end {
                let inner = &text[i + needle.len()..end];

                let mut parts: Vec<String> = Vec::new();
                let mut d = 0i32;
                let mut cur = String::new();
                for ch in inner.chars() {
                    match ch {
                        '<' | '(' | '[' => d += 1,
                        '>' | ')' | ']' => d -= 1,
                        _ => {}
                    }
                    if ch == ',' && d == 0 {
                        parts.push(std::mem::take(&mut cur));
                    } else {
                        cur.push(ch);
                    }
                }
                parts.push(cur);
                if parts.len() == 2 {
                    out.push(parts[1].trim().to_string());
                }
                i = end + 1;
                continue;
            }
        }
        i += 1;
    }
    out
}

#[test]
fn no_production_result_string_error_seam() {
    let mut files = Vec::new();
    rust_files(&src_dir(), &mut files);
    let mut offenders = Vec::new();
    for path in files {
        if !is_gated_production(&path) {
            continue;
        }
        let raw = fs::read_to_string(&path).unwrap_or_default();

        let text: String = raw
            .lines()
            .filter(|line| !line.trim_start().starts_with("//"))
            .collect::<Vec<_>>()
            .join("\n");
        for err_ty in result_error_types(&text) {
            if err_ty == "String" {
                offenders.push(path.display().to_string());
                break;
            }
        }
    }
    assert!(
        offenders.is_empty(),
        "production `Result<_, String>` error seam(s) reintroduced in: {offenders:?}. \
         Return `PactResult<_>` and construct an explicit `PactError` kind."
    );
}

#[test]
fn no_broad_string_to_pact_error_conversion() {
    let error_rs = fs::read_to_string(src_dir().join("error.rs")).expect("read error.rs");

    let code: String = error_rs
        .lines()
        .filter(|line| !line.trim_start().starts_with("//"))
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        !code.contains("impl From<String> for PactError"),
        "broad `impl From<String> for PactError` reintroduced — construct an \
         explicit error kind instead of relying on string conversion."
    );
    assert!(
        !code.contains("impl From<&str> for PactError"),
        "broad `impl From<&str> for PactError` reintroduced — construct an \
         explicit error kind instead of relying on string conversion."
    );
}
