use clap::Parser;
use pactree::{PactError, PactResult, file_format};
use serde_json::Value;
use std::fs::{self, File};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};

const PRE_V01_MAGIC: &[u8; 8] = b"PACTBIN1";

#[derive(Parser)]
#[command(name = "pact-v01-convert", version, about)]
struct Cli {
    #[arg(long, value_name = "PATH", hide = true)]
    pre_v01_pactree: Option<PathBuf>,
    #[arg(value_name = "SOURCE.pact")]
    source: PathBuf,
    #[arg(value_name = "OUTPUT.pact")]
    output: PathBuf,
}

fn main() -> ExitCode {
    match run(Cli::parse()) {
        Ok(report) => {
            println!("{report}");
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("pact-v01-convert: {error}");
            ExitCode::FAILURE
        }
    }
}

fn run(cli: Cli) -> PactResult<String> {
    if cli.output.exists() {
        return Err(PactError::invalid(format!(
            "conversion output already exists: {}",
            cli.output.display()
        )));
    }
    let source_before = source_fingerprint(&cli.source)?;
    verify_pre_v01_magic(&cli.source)?;

    let report_json = match file_format::file_convert_pre_v01_file(&cli.source, &cli.output) {
        Ok(report) => report,
        Err(direct_error) => {
            let Some(pre_v01_pactree) = &cli.pre_v01_pactree else {
                return Err(direct_error);
            };
            let export = Command::new(pre_v01_pactree)
                .arg("file-records")
                .arg(&cli.source)
                .output()
                .map_err(|error| {
                    PactError::storage(format!(
                        "direct pre-v0.1 decode failed ({direct_error}); fallback reader failed: {error}"
                    ))
                })?;
            if !export.status.success() {
                return Err(direct_error);
            }
            let exported_records = String::from_utf8(export.stdout).map_err(|error| {
                PactError::codec(format!("pre-v0.1 record export is not UTF-8: {error}"))
            })?;
            file_format::file_convert_exported_records_json_owned(exported_records, &cli.output)?
        }
    };

    let source_after = source_fingerprint(&cli.source)?;
    if source_before != source_after {
        fs::remove_file(&cli.output).ok();
        return Err(PactError::storage(
            "pre-v0.1 source changed while conversion was running",
        ));
    }

    let mut report: Value = serde_json::from_str(&report_json)
        .map_err(|error| PactError::codec(format!("invalid conversion report: {error}")))?;
    report["source_path"] = Value::String(cli.source.display().to_string());
    report["source_bytes"] = Value::Number(source_before.0.into());
    report["source_digest"] = Value::String(source_before.1);
    serde_json::to_string_pretty(&report)
        .map_err(|error| PactError::codec(format!("encode converter report: {error}")))
}

fn verify_pre_v01_magic(path: &Path) -> PactResult<()> {
    let mut file = File::open(path).map_err(|error| {
        PactError::storage(format!("failed to open source {}: {error}", path.display()))
    })?;
    let mut magic = [0_u8; 8];
    file.read_exact(&mut magic).map_err(|error| {
        PactError::storage(format!(
            "failed to read source magic {}: {error}",
            path.display()
        ))
    })?;
    if &magic != PRE_V01_MAGIC {
        return Err(PactError::invalid(format!(
            "source is not a PACTBIN1 file: {}",
            path.display()
        )));
    }
    Ok(())
}

fn source_fingerprint(path: &Path) -> PactResult<(u64, String)> {
    let mut file = File::open(path).map_err(|error| {
        PactError::storage(format!("failed to open source {}: {error}", path.display()))
    })?;
    let mut hasher = blake3::Hasher::new();
    let mut bytes = 0_u64;
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let read = file.read(&mut buffer).map_err(|error| {
            PactError::storage(format!("failed to read source {}: {error}", path.display()))
        })?;
        if read == 0 {
            break;
        }
        bytes += read as u64;
        hasher.update(&buffer[..read]);
    }
    Ok((bytes, hasher.finalize().to_hex().to_string()))
}
