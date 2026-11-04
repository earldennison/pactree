use crate::error::{PactError, PactResult};
use crate::file_format;
use clap::Parser;
use std::ffi::OsString;
use std::path::PathBuf;

#[cfg(test)]
mod tests;

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Command {
    Validate {
        path: PathBuf,
    },
    Rewind {
        path: PathBuf,
        branch_id: Option<String>,
        revision: u64,
    },
    Repair {
        source: PathBuf,
        output: PathBuf,
        truncate_tail: bool,
    },
    Version,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Cli {
    pub command: Command,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CommandOutcome {
    pub stdout: String,
    pub stderr: String,
    pub success: bool,
}

#[derive(Debug, Parser)]
#[command(name = "pactree", disable_help_flag = true)]
struct ValidationArgs {
    path: PathBuf,
}

#[derive(Debug, Parser)]
#[command(name = "pactree", disable_help_flag = true)]
struct RewindArgs {
    path: PathBuf,
    branch_or_revision: String,
    revision: Option<u64>,
}

#[derive(Debug, Parser)]
#[command(name = "pactree", disable_help_flag = true)]
struct RepairArgs {
    source: PathBuf,
    output: Option<PathBuf>,
    #[arg(long)]
    out: Option<PathBuf>,
    #[arg(long)]
    truncate_tail: bool,
}

pub fn parse_known(args: impl IntoIterator<Item = OsString>) -> PactResult<Option<Cli>> {
    let args = args.into_iter().collect::<Vec<_>>();
    let Some(command) = args.get(1).and_then(|value| value.to_str()) else {
        return Ok(None);
    };
    let command = match command {
        "file-validate" | "check" | "doctor" => {
            let parsed = ValidationArgs::try_parse_from(
                std::iter::once(args[0].clone()).chain(args.iter().skip(2).cloned()),
            )
            .map_err(|error| PactError::invalid(error.to_string()))?;
            Command::Validate { path: parsed.path }
        }
        "file-rewind-to" | "rewind-to" => {
            let parsed = RewindArgs::try_parse_from(
                std::iter::once(args[0].clone()).chain(args.iter().skip(2).cloned()),
            )
            .map_err(|error| PactError::invalid(error.to_string()))?;
            let (branch_id, revision) = match parsed.revision {
                Some(revision) => (Some(parsed.branch_or_revision), revision),
                None => (
                    None,
                    parsed.branch_or_revision.parse::<u64>().map_err(|error| {
                        PactError::invalid(format!("invalid rewind revision: {error}"))
                    })?,
                ),
            };
            Command::Rewind {
                path: parsed.path,
                branch_id,
                revision,
            }
        }
        "file-repair" | "heal" => {
            let parsed = RepairArgs::try_parse_from(
                std::iter::once(args[0].clone()).chain(args.iter().skip(2).cloned()),
            )
            .map_err(|error| PactError::invalid(error.to_string()))?;
            let output = match (parsed.output, parsed.out) {
                (Some(output), None) | (None, Some(output)) => output,
                (Some(_), Some(_)) => {
                    return Err(PactError::invalid(
                        "repair output must be positional or --out, not both",
                    ));
                }
                (None, None) => {
                    return Err(PactError::invalid(
                        "file-repair/heal requires an output .pact path",
                    ));
                }
            };
            Command::Repair {
                source: parsed.source,
                output,
                truncate_tail: parsed.truncate_tail,
            }
        }
        "--version" | "-V" => Command::Version,
        _ => return Ok(None),
    };
    Ok(Some(Cli { command }))
}

pub fn execute(command: Command) -> PactResult<CommandOutcome> {
    match command {
        Command::Validate { path } => {
            let validation = file_format::validate_file(&path)?;
            let success = validation.ok;
            let stdout = serde_json::to_string_pretty(&validation).map_err(|error| {
                PactError::codec(format!("failed to encode .pact validation: {error}"))
            })?;
            Ok(CommandOutcome {
                stdout,
                stderr: String::new(),
                success,
            })
        }
        Command::Rewind {
            path,
            branch_id,
            revision,
        } => Ok(CommandOutcome {
            stdout: file_format::file_rewind_to_json(
                &path,
                branch_id.as_deref(),
                revision,
                blob_inline_limit(),
            )?,
            stderr: String::new(),
            success: true,
        }),
        Command::Repair {
            source,
            output,
            truncate_tail,
        } => {
            let report = file_format::repair_file(&source, &output, truncate_tail)?;
            let success = report.repaired_validation.ok;
            let stdout = serde_json::to_string_pretty(&report).map_err(|error| {
                PactError::codec(format!("failed to encode .pact repair report: {error}"))
            })?;
            Ok(CommandOutcome {
                stdout,
                stderr: String::new(),
                success,
            })
        }
        Command::Version => Ok(CommandOutcome {
            stdout: format!("pactree {}", env!("CARGO_PKG_VERSION")),
            stderr: String::new(),
            success: true,
        }),
    }
}

fn blob_inline_limit() -> usize {
    std::env::var("PACT_BLOB_INLINE_LIMIT")
        .ok()
        .and_then(|value| value.parse::<usize>().ok())
        .unwrap_or(1024)
}
