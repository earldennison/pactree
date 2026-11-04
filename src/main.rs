use pactree::{
    Component, DEFAULT_PREVIEW_INLINE_LIMIT_BYTES, PactAppendWriter, PactError, PactMemoryOptions,
    PactResult,
    atomic_output::{PublishMode, publish_bytes},
    file_format, inspect, normalize_pact_json, normalize_tree, selector,
};
use std::env;
use std::fs;
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode, Stdio};

fn main() -> ExitCode {
    match pactree::cli::parse_known(env::args_os()) {
        Ok(Some(cli)) => match pactree::cli::execute(cli.command) {
            Ok(outcome) => {
                if !outcome.stdout.is_empty() {
                    println!("{}", outcome.stdout);
                }
                if !outcome.stderr.is_empty() {
                    eprintln!("{}", outcome.stderr);
                }
                if outcome.success {
                    ExitCode::SUCCESS
                } else {
                    ExitCode::FAILURE
                }
            }
            Err(err) => report_error(err),
        },
        Ok(None) => match run() {
            Ok(()) => ExitCode::SUCCESS,
            Err(err) => report_error(err),
        },
        Err(err) => report_error(err),
    }
}

fn report_error(err: PactError) -> ExitCode {
    eprintln!("pactree: {err}");
    ExitCode::FAILURE
}

fn run() -> PactResult<()> {
    let command = env::args().nth(1).unwrap_or_else(|| "help".to_string());
    match command.as_str() {
        "normalize" => {
            let input = read_stdin()?;
            let normalized = normalize_pact_json(&input, blob_inline_limit());
            println!("{}", normalized?);
            Ok(())
        }
        "inspect" => {
            let sub = env::args().nth(2);
            match sub.as_deref() {
                Some("timeline") => {
                    let path = env::args().nth(3).ok_or_else(|| {
                        PactError::invalid("inspect timeline requires a .pact path")
                    })?;
                    println!("{}", file_format::file_timeline_json(Path::new(&path))?);
                }
                Some(path) => {
                    println!("{}", file_format::file_inspect_json(Path::new(path))?);
                }
                None => {
                    let root = read_node()?;
                    let normalized = normalize_tree(root);
                    println!(
                        "{}",
                        serde_json::to_string_pretty(&inspect(&normalized))
                            .map_err(|err| PactError::codec(err.to_string()))?
                    );
                }
            }
            Ok(())
        }
        "select" => {
            let first = env::args().nth(2).ok_or_else(|| {
                PactError::invalid(
                    "select requires a selector (over stdin) or a .pact path plus a selector"
                        .to_string(),
                )
            })?;
            if first.ends_with(".pact") || Path::new(&first).is_file() {
                let args = parse_file_select_args()?;
                let output = match args.parent_config {
                    Some(parent_config) => {
                        let result = file_format::file_select_with_parents(
                            Path::new(&args.path),
                            args.query,
                            &parent_config,
                        )?;
                        serde_json::to_string_pretty(&result)
                            .map_err(|err| PactError::codec(err.to_string()))?
                    }
                    None => file_format::file_select_json(Path::new(&args.path), args.query)?,
                };
                println!("{output}");
            } else {
                let input = read_stdin()?;
                println!("{}", selector::select_json(&input, &first)?);
            }
            Ok(())
        }
        "prune" => {
            let selector_arg = env::args()
                .nth(2)
                .ok_or_else(|| PactError::invalid("prune requires a selector argument"))?;
            let branch_id = env::args().nth(3).unwrap_or_else(|| "main".to_string());
            let input = read_stdin()?;
            println!(
                "{}",
                cli_prune_ledger_json(&input, &branch_id, &selector_arg)?
            );
            Ok(())
        }
        "store-new" => {
            let input = read_stdin()?;
            println!("{}", pactree::new_store_json(&input, blob_inline_limit())?);
            Ok(())
        }
        "root-new" => {
            let mut no_base = false;
            let mut branch_id = None;
            let mut inline_limit = blob_inline_limit();
            let mut args = env::args().skip(2);
            while let Some(arg) = args.next() {
                match arg.as_str() {
                    "--no-base" => no_base = true,
                    "--payload-profile" => {
                        let profile = args.next().ok_or_else(|| {
                            PactError::invalid(
                                "--payload-profile requires hot, archive, or debug".to_string(),
                            )
                        })?;
                        inline_limit = payload_inline_limit_for_profile(&profile)?;
                    }
                    _ if branch_id.is_none() => branch_id = Some(arg),
                    _ => {
                        return Err(PactError::invalid(
                            "root-new accepts at most one branch id".to_string(),
                        ));
                    }
                }
            }
            let input = read_stdin()?;
            let root_store_json = if no_base {
                pactree::new_ledger_json_without_base(&input, branch_id.as_deref(), inline_limit)?
            } else {
                pactree::new_ledger_json(&input, branch_id.as_deref(), inline_limit)?
            };
            println!("{root_store_json}");
            Ok(())
        }
        "root-from-store" => {
            let branch_id = env::args().nth(2);
            let input = read_stdin()?;
            println!(
                "{}",
                pactree::ledger_from_store_json(&input, branch_id.as_deref())?
            );
            Ok(())
        }
        "root-branch-revision" => {
            let from_branch_id = env::args().nth(2).ok_or_else(|| {
                PactError::invalid("root-branch-revision requires a source branch id")
            })?;
            let revision = env::args()
                .nth(3)
                .map(|value| {
                    value
                        .parse::<u64>()
                        .map_err(|error| PactError::invalid(format!("invalid revision: {error}")))
                })
                .transpose()?;
            let new_branch_id = env::args().nth(4);
            let input = read_stdin()?;
            println!(
                "{}",
                pactree::ledger_create_branch_at_revision_json(
                    &input,
                    &from_branch_id,
                    revision,
                    new_branch_id.as_deref(),
                    None,
                    blob_inline_limit(),
                )?
            );
            Ok(())
        }
        "root-branch-component" => {
            let from_branch_id = env::args().nth(2).ok_or_else(|| {
                PactError::invalid("root-branch-component requires a source branch id")
            })?;
            let component_id = env::args().nth(3).ok_or_else(|| {
                PactError::invalid("root-branch-component requires a component id")
            })?;
            let new_branch_id = env::args().nth(4);
            let input = read_stdin()?;
            println!(
                "{}",
                pactree::ledger_create_branch_at_node_json(
                    &input,
                    &from_branch_id,
                    &component_id,
                    new_branch_id.as_deref(),
                    None,
                    blob_inline_limit(),
                )?
            );
            Ok(())
        }
        "root-record-transaction" => {
            let trace_arg = env::args().nth(2).ok_or_else(|| {
                PactError::invalid(
                    "root-record-transaction requires a transaction trace JSON argument"
                        .to_string(),
                )
            })?;
            let input = read_stdin()?;
            println!(
                "{}",
                pactree::ledger_record_transaction_trace_json(&input, &trace_arg)?
            );
            Ok(())
        }
        "root-branch-transaction" => {
            let from_branch_id = env::args().nth(2).ok_or_else(|| {
                PactError::invalid("root-branch-transaction requires a source branch id")
            })?;
            let transaction_id = env::args().nth(3).ok_or_else(|| {
                PactError::invalid("root-branch-transaction requires a transaction id")
            })?;
            let new_branch_id = env::args().nth(4);
            let input = read_stdin()?;
            println!(
                "{}",
                pactree::ledger_create_branch_at_transaction_json(
                    &input,
                    &from_branch_id,
                    &transaction_id,
                    new_branch_id.as_deref(),
                    None,
                    blob_inline_limit(),
                )?
            );
            Ok(())
        }
        "root-project-branch" => {
            let branch_id = env::args()
                .nth(2)
                .ok_or_else(|| PactError::invalid("root-project-branch requires a branch id"))?;
            let revision = env::args()
                .nth(3)
                .map(|value| {
                    value
                        .parse::<u64>()
                        .map_err(|error| PactError::invalid(format!("invalid revision: {error}")))
                })
                .transpose()?;
            let input = read_stdin()?;
            println!(
                "{}",
                pactree::ledger_project_branch_json(
                    &input,
                    &branch_id,
                    revision,
                    blob_inline_limit(),
                )?
            );
            Ok(())
        }
        "root-inspect" => {
            let input = read_stdin()?;
            println!("{}", pactree::ledger_inspect_json(&input)?);
            Ok(())
        }
        "root-inspect-branch" => {
            let branch_id = env::args()
                .nth(2)
                .ok_or_else(|| PactError::invalid("root-inspect-branch requires a branch id"))?;
            let input = read_stdin()?;
            println!(
                "{}",
                pactree::ledger_inspect_branch_json(&input, &branch_id)?
            );
            Ok(())
        }
        "store-project" => {
            let revision = env::args()
                .nth(2)
                .map(|value| {
                    value
                        .parse::<u64>()
                        .map_err(|error| PactError::invalid(format!("invalid revision: {error}")))
                })
                .transpose()?;
            let input = read_stdin()?;
            println!("{}", pactree::project_store_json(&input, revision)?);
            Ok(())
        }
        "content-put" => {
            let kind = env::args().nth(2).ok_or_else(|| {
                PactError::invalid("content-put requires a content kind argument")
            })?;
            let content = read_stdin()?;
            println!(
                "{}",
                pactree::content_store_put_inline_json(None, &kind, &content, blob_inline_limit())?
            );
            Ok(())
        }
        "transaction-validate" => {
            let input = read_stdin()?;
            println!("{}", pactree::transaction_trace_validate_json(&input)?);
            Ok(())
        }
        "file-create" => {
            let mut path = None;
            let mut include_previews = false;
            let mut preview_limit = DEFAULT_PREVIEW_INLINE_LIMIT_BYTES;
            let mut token_span_strings = true;
            let mut args = env::args().skip(2);
            while let Some(arg) = args.next() {
                match arg.as_str() {
                    "--include-previews" => include_previews = true,
                    "--token-span" => token_span_strings = true,
                    "--no-token-span" => token_span_strings = false,
                    "--preview-limit" => {
                        preview_limit = args
                            .next()
                            .ok_or_else(|| {
                                PactError::invalid("--preview-limit requires a byte count")
                            })?
                            .parse::<usize>()
                            .map_err(|_| {
                                PactError::invalid("invalid --preview-limit byte count")
                            })?;
                    }
                    _ if path.is_none() => path = Some(arg),
                    _ => {
                        return Err(PactError::invalid(format!(
                            "unexpected file-create argument: {arg}"
                        )));
                    }
                }
            }
            let path = path
                .ok_or_else(|| PactError::invalid("file-create requires an output .pact path"))?;
            let input = read_stdin()?;
            let report = if include_previews {
                file_format::file_create_from_ledger_with_previews_and_token_span_json(
                    Path::new(&path),
                    &input,
                    preview_limit,
                    token_span_strings,
                )?
            } else {
                file_format::file_create_from_ledger_with_token_span_json(
                    Path::new(&path),
                    &input,
                    token_span_strings,
                )?
            };
            println!("{report}");
            Ok(())
        }
        "file-repack" | "repack" => {
            let mut source = None;
            let mut output = None;
            let mut token_span_strings = true;
            let mut packed_v1 = false;
            let mut args = env::args().skip(2);
            while let Some(arg) = args.next() {
                match arg.as_str() {
                    "--token-span" => token_span_strings = true,
                    "--no-token-span" => token_span_strings = false,
                    "--format" => {
                        let value = args
                            .next()
                            .ok_or_else(|| PactError::invalid("--format requires a value"))?;
                        match value.as_str() {
                            "packed-v1" => packed_v1 = true,
                            "v0" | "record-pages-v0" => packed_v1 = false,
                            other => {
                                return Err(PactError::invalid(format!(
                                    "unknown file-repack format: {other} (expected v0 or packed-v1)"
                                )));
                            }
                        }
                    }
                    value if source.is_none() => source = Some(value.to_string()),
                    value if output.is_none() => output = Some(value.to_string()),
                    value => {
                        return Err(PactError::invalid(format!(
                            "unexpected file-repack argument: {value}"
                        )));
                    }
                }
            }
            let source = source
                .ok_or_else(|| PactError::invalid("file-repack requires a source .pact path"))?;
            let output = output
                .ok_or_else(|| PactError::invalid("file-repack requires an output .pact path"))?;
            let report = if packed_v1 {
                file_format::file_repack_packed_v1_json(Path::new(&source), Path::new(&output))?
            } else {
                file_format::file_repack_with_options_json(
                    Path::new(&source),
                    Path::new(&output),
                    file_format::PactRepackOptions { token_span_strings },
                )?
            };
            println!("{report}");
            Ok(())
        }
        "file-inspect" => {
            let args = env::args().skip(2).collect::<Vec<_>>();
            let full = args.iter().any(|arg| arg == "--full" || arg == "--records");
            let path = args
                .iter()
                .find(|arg| !matches!(arg.as_str(), "--full" | "--records" | "--json"))
                .ok_or_else(|| PactError::invalid("file inspect requires a .pact path"))?;
            if full {
                println!("{}", file_format::file_inspect_full_json(Path::new(path))?);
            } else {
                println!("{}", file_format::file_inspect_json(Path::new(path))?);
            }
            Ok(())
        }
        "file-order-check" => {
            let path = env::args()
                .nth(2)
                .ok_or_else(|| PactError::invalid("file order check requires a .pact path"))?;
            println!(
                "{}",
                file_format::file_message_order_report(Path::new(&path))?
            );
            Ok(())
        }
        "file-order-repair" => {
            let args = env::args().skip(2).collect::<Vec<_>>();
            let confirmed = args.iter().any(|arg| arg == "--i-have-a-copy");
            let branch = args
                .windows(2)
                .find(|pair| pair[0] == "--branch")
                .map(|pair| pair[1].clone())
                .unwrap_or_else(|| "main".to_string());
            let path = args
                .iter()
                .enumerate()
                .find(|(index, arg)| {
                    !arg.starts_with("--") && (*index == 0 || args[index - 1] != "--branch")
                })
                .map(|(_, arg)| arg.clone())
                .ok_or_else(|| PactError::invalid("file order repair requires a .pact path"))?;
            if !confirmed {
                return Err(PactError::invalid(
                    "file-order-repair rewrites the derived index of the file in place; run it on a copy and pass --i-have-a-copy",
                ));
            }
            let started = std::time::Instant::now();
            let mut writer =
                PactAppendWriter::open(Path::new(&path), &branch, PactMemoryOptions::default())
                    .map_err(|failure| {
                        PactError::invalid(format!("{}: {failure}", failure.category()))
                    })?;

            writer.set_automatic_maintenance(false);
            let opened_ms = started.elapsed().as_millis();
            let repair = writer.repair_message_order().map_err(|failure| {
                PactError::invalid(format!("{}: {failure}", failure.category()))
            })?;
            let repaired_ms = started.elapsed().as_millis();
            writer.close();
            let order_check = file_format::file_message_order_report(Path::new(&path))?;
            let checked_ms = started.elapsed().as_millis();
            println!(
                "{}",
                serde_json::json!({
                    "schema": "pact.message_order_repair.v1",
                    "path": path,
                    "branch": branch,
                    "repair": {
                        "transaction_id": repair.transaction_id,
                        "revision": repair.revision,
                        "rows_scanned": repair.rows_scanned,
                        "rows_reindexed": repair.rows_reindexed,
                        "stale_keys_removed": repair.stale_keys_removed,
                    },
                    "elapsed_ms": {
                        "open": opened_ms,
                        "repair": repaired_ms - opened_ms,
                        "order_check": checked_ms - repaired_ms,
                    },
                    "order_check": order_check,
                })
            );
            Ok(())
        }
        "token-span-stats" | "file-token-span-stats" => {
            let path = env::args()
                .nth(2)
                .ok_or_else(|| PactError::invalid("token-span-stats requires a .pact path"))?;
            println!(
                "{}",
                file_format::file_token_span_stats_json(Path::new(&path))?
            );
            Ok(())
        }
        "page-compare" | "file-page-compare" => {
            let baseline = env::args()
                .nth(2)
                .ok_or_else(|| PactError::invalid("page-compare requires a baseline .pact path"))?;
            let candidate = env::args().nth(3).ok_or_else(|| {
                PactError::invalid("page-compare requires a candidate .pact path")
            })?;
            println!(
                "{}",
                file_format::file_page_compare_json(Path::new(&baseline), Path::new(&candidate))?
            );
            Ok(())
        }
        "file-records" => {
            let path = env::args()
                .nth(2)
                .ok_or_else(|| PactError::invalid("file-records requires a .pact path"))?;
            let filter_kind = env::args().nth(3);
            let bytes = std::fs::read(&path)
                .map_err(|err| PactError::storage(format!("failed to read .pact file: {err}")))?;
            let scanned = file_format::scan_file(&bytes, true);
            if !scanned.errors.is_empty() {
                return Err(PactError::corrupt(scanned.errors.join("; ")));
            }
            let records = scanned
                .records
                .iter()
                .filter(|record| {
                    filter_kind
                        .as_deref()
                        .is_none_or(|kind| kind == record.kind)
                })
                .map(|record| {
                    serde_json::json!({
                        "sequence": record.sequence,
                        "kind": record.kind,
                        "payload_bytes": record.payload_bytes,
                        "payload": record.payload,
                    })
                })
                .collect::<Vec<_>>();
            println!(
                "{}",
                serde_json::to_string_pretty(&records)
                    .map_err(|err| PactError::codec(err.to_string()))?
            );
            Ok(())
        }
        "info" | "file-info" | "pactree-info" => {
            let args = env::args().skip(2).collect::<Vec<_>>();
            let manifest_only = args.iter().any(|arg| arg == "--manifest-only");
            let live_shape_only = args.iter().any(|arg| arg == "--live-shape");
            let path = args
                .iter()
                .find(|arg| !matches!(arg.as_str(), "--json" | "--manifest-only" | "--live-shape"))
                .ok_or_else(|| PactError::invalid("file info requires a .pact path"))?;
            if live_shape_only {
                println!(
                    "{}",
                    file_format::file_live_shape_info_json(Path::new(path))?
                );
            } else if manifest_only {
                println!("{}", file_format::file_manifest_info_json(Path::new(path))?);
            } else {
                println!("{}", file_format::file_info_json(Path::new(path))?);
            }
            Ok(())
        }
        "storage-audit" | "file-audit" | "file-storage-audit" => {
            let path = env::args()
                .skip(2)
                .find(|arg| arg != "--json")
                .ok_or_else(|| PactError::invalid("storage-audit requires a .pact path"))?;
            println!(
                "{}",
                file_format::file_storage_audit_json(Path::new(&path))?
            );
            Ok(())
        }
        "bench-storage" | "file-bench-storage" => {
            let mut path = None;
            let mut target_profile = None;
            let mut args = env::args().skip(2);
            while let Some(arg) = args.next() {
                match arg.as_str() {
                    "--json" => {}
                    "--target-profile" => {
                        target_profile = Some(args.next().ok_or_else(|| {
                            PactError::invalid(
                                "--target-profile requires a profile name".to_string(),
                            )
                        })?);
                    }
                    value if !value.starts_with("--") => path = Some(value.to_string()),
                    value => {
                        return Err(PactError::invalid(format!(
                            "unknown bench-storage argument: {value}"
                        )));
                    }
                }
            }
            let path =
                path.ok_or_else(|| PactError::invalid("bench-storage requires a .pact path"))?;
            println!(
                "{}",
                file_format::file_bench_storage_with_profile_json(
                    Path::new(&path),
                    target_profile.as_deref(),
                )?
            );
            Ok(())
        }
        "file-fast-head" => {
            let path = env::args()
                .nth(2)
                .ok_or_else(|| PactError::invalid("file-fast-head requires a .pact path"))?;
            println!(
                "{}",
                file_format::file_open_head_fast_json(Path::new(&path))?
            );
            Ok(())
        }
        "fork" => run_fork_command(),
        "send-message" => run_send_message_command(),
        "file-append-commit" => {
            let path = env::args()
                .nth(2)
                .ok_or_else(|| PactError::invalid("file-append-commit requires a .pact path"))?;
            let commit_path = env::args().nth(3).ok_or_else(|| {
                PactError::invalid("file-append-commit requires a commit JSON path")
            })?;
            let nodes_path = env::args().nth(4).ok_or_else(|| {
                PactError::invalid(
                    "file-append-commit requires a node records JSON path".to_string(),
                )
            })?;
            let commit = std::fs::read_to_string(&commit_path)
                .map_err(|err| PactError::storage(format!("failed to read commit JSON: {err}")))?;
            let nodes = std::fs::read_to_string(&nodes_path).map_err(|err| {
                PactError::storage(format!("failed to read node records JSON: {err}"))
            })?;
            println!(
                "{}",
                file_format::file_append_commit_json(Path::new(&path), &commit, &nodes)?
            );
            Ok(())
        }
        "no-anchor" | "no-anchors" | "strip-anchors" => {
            let source = env::args()
                .nth(2)
                .ok_or_else(|| PactError::invalid("no-anchor requires a source .pact path"))?;
            let mut output = env::args().nth(3);
            let mut args = env::args().skip(3);
            while let Some(arg) = args.next() {
                match arg.as_str() {
                    "--out" => {
                        output = Some(args.next().ok_or_else(|| {
                            PactError::invalid("--out requires an output .pact path".to_string())
                        })?);
                    }
                    value if !value.starts_with("--") => output = Some(value.to_string()),
                    value => {
                        return Err(PactError::invalid(format!(
                            "unknown no-anchor argument: {value}"
                        )));
                    }
                }
            }
            let output = output
                .ok_or_else(|| PactError::invalid("no-anchor requires an output .pact path"))?;
            println!(
                "{}",
                file_format::file_reanchor_json(
                    Path::new(&source),
                    Path::new(&output),
                    file_format::ReanchorOptions::no_anchors(),
                )?
            );
            Ok(())
        }
        "file-reanchor" | "reanchor" | "reindex-anchors" => {
            let source = env::args()
                .nth(2)
                .ok_or_else(|| PactError::invalid("file-reanchor requires a source .pact path"))?;
            let mut output = env::args().nth(3);
            let mut reanchor_options = file_format::ReanchorOptions::default();
            let mut args = env::args().skip(3);
            while let Some(arg) = args.next() {
                match arg.as_str() {
                    "--out" => {
                        output = Some(args.next().ok_or_else(|| {
                            PactError::invalid("--out requires an output .pact path".to_string())
                        })?);
                    }
                    "--freeze-commit-bytes" | "--max-bytes" => {
                        let value = args.next().ok_or_else(|| {
                            PactError::invalid(format!("{arg} requires a byte threshold"))
                        })?;
                        reanchor_options.freeze_commit_bytes =
                            Some(value.parse::<usize>().map_err(|_| {
                                PactError::invalid(format!("invalid {arg} value: {value}"))
                            })?);
                    }
                    "--freeze-cycle-interval" | "--every-cycles" => {
                        let value = args.next().ok_or_else(|| {
                            PactError::invalid(format!("{arg} requires a cycle count"))
                        })?;
                        reanchor_options.freeze_cycle_interval =
                            Some(value.parse::<u64>().map_err(|_| {
                                PactError::invalid(format!("invalid {arg} value: {value}"))
                            })?);
                    }
                    "--no-anchors" => reanchor_options = file_format::ReanchorOptions::no_anchors(),
                    value if !value.starts_with("--") => output = Some(value.to_string()),
                    value => {
                        return Err(PactError::invalid(format!(
                            "unknown file-reanchor argument: {value}"
                        )));
                    }
                }
            }
            let output = output
                .ok_or_else(|| PactError::invalid("file-reanchor requires an output .pact path"))?;
            println!(
                "{}",
                file_format::file_reanchor_json(
                    Path::new(&source),
                    Path::new(&output),
                    reanchor_options,
                )?
            );
            Ok(())
        }
        "reindex-head" | "file-reindex-head" => {
            let source = env::args()
                .nth(2)
                .ok_or_else(|| PactError::invalid("reindex-head requires a source .pact path"))?;
            let mut output = None;
            let mut args = env::args().skip(3);
            while let Some(arg) = args.next() {
                match arg.as_str() {
                    "--json" => {}
                    "--output" => {
                        output =
                            Some(args.next().ok_or_else(|| {
                                PactError::invalid("--output requires a .pact path")
                            })?);
                    }
                    value if !value.starts_with("--") => output = Some(value.to_string()),
                    value => {
                        return Err(PactError::invalid(format!(
                            "unknown reindex-head argument: {value}"
                        )));
                    }
                }
            }
            println!(
                "{}",
                file_format::file_reindex_head_json(
                    Path::new(&source),
                    output.as_deref().map(Path::new),
                )?
            );
            Ok(())
        }
        "bundle" => {
            let action = env::args().nth(2).ok_or_else(|| {
                PactError::invalid("bundle requires create|verify|export-full|prune")
            })?;
            let path = env::args()
                .nth(3)
                .ok_or_else(|| PactError::invalid("bundle command requires a .pact path"))?;
            match action.as_str() {
                "create" => {
                    println!(
                        "{}",
                        file_format::file_bundle_create_json(Path::new(&path))?
                    );
                    Ok(())
                }
                "verify" => {
                    println!(
                        "{}",
                        file_format::file_bundle_verify_json(Path::new(&path))?
                    );
                    Ok(())
                }
                "export-full" => {
                    let output = env::args().nth(4).ok_or_else(|| {
                        PactError::invalid(
                            "bundle export-full requires an output .pact path".to_string(),
                        )
                    })?;
                    println!(
                        "{}",
                        file_format::file_bundle_export_full_json(
                            Path::new(&path),
                            Path::new(&output),
                        )?
                    );
                    Ok(())
                }
                "prune" => {
                    println!("{}", file_format::file_bundle_prune_json(Path::new(&path))?);
                    Ok(())
                }
                other => Err(PactError::invalid(format!(
                    "unsupported bundle command: {other}"
                ))),
            }
        }
        "file-bundle-create" => {
            let path = env::args()
                .nth(2)
                .ok_or_else(|| PactError::invalid("file-bundle-create requires a .pact path"))?;
            println!(
                "{}",
                file_format::file_bundle_create_json(Path::new(&path))?
            );
            Ok(())
        }
        "file-bundle-verify" => {
            let path = env::args()
                .nth(2)
                .ok_or_else(|| PactError::invalid("file-bundle-verify requires a .pact path"))?;
            println!(
                "{}",
                file_format::file_bundle_verify_json(Path::new(&path))?
            );
            Ok(())
        }
        "file-bundle-export-full" => {
            let path = env::args().nth(2).ok_or_else(|| {
                PactError::invalid("file-bundle-export-full requires a .pact path")
            })?;
            let output = env::args().nth(3).ok_or_else(|| {
                PactError::invalid(
                    "file-bundle-export-full requires an output .pact path".to_string(),
                )
            })?;
            println!(
                "{}",
                file_format::file_bundle_export_full_json(Path::new(&path), Path::new(&output))?
            );
            Ok(())
        }
        "file-bundle-prune" => {
            let path = env::args()
                .nth(2)
                .ok_or_else(|| PactError::invalid("file-bundle-prune requires a .pact path"))?;
            println!("{}", file_format::file_bundle_prune_json(Path::new(&path))?);
            Ok(())
        }
        "extension" | "extensions" => run_extension_command(),
        "search" => run_search_command(),
        "file-append-content-manifest" => {
            let path = env::args().nth(2).ok_or_else(|| {
                PactError::invalid("file-append-content-manifest requires a .pact path")
            })?;
            let input = read_stdin()?;
            println!(
                "{}",
                file_format::file_append_content_manifest_json(Path::new(&path), &input)?
            );
            Ok(())
        }
        "file-content-manifest-payload" => {
            let path = env::args().nth(2).ok_or_else(|| {
                PactError::invalid("file-content-manifest-payload requires a .pact path")
            })?;
            let manifest_id = env::args().nth(3).ok_or_else(|| {
                PactError::invalid(
                    "file-content-manifest-payload requires a manifest id".to_string(),
                )
            })?;
            println!(
                "{}",
                file_format::file_content_manifest_payload_json(Path::new(&path), &manifest_id)?
            );
            Ok(())
        }
        "file-append-views" => {
            let path = env::args()
                .nth(2)
                .ok_or_else(|| PactError::invalid("file-append-views requires a .pact path"))?;
            let input = read_stdin()?;
            println!(
                "{}",
                file_format::file_append_view_records_json(
                    Path::new(&path),
                    &input,
                    blob_inline_limit(),
                )?
            );
            Ok(())
        }
        "branch-view" => run_view_command(),
        "view" => run_view_presentation_command(),
        "load" => run_load_command(),
        "slice" => {
            let range = env::args()
                .nth(2)
                .ok_or_else(|| PactError::invalid("slice requires a cycle range, e.g. 5..29"))?;
            let parsed_range = file_format::parse_pact_range(&range)?;
            let source = env::args()
                .nth(3)
                .ok_or_else(|| PactError::invalid("slice requires a source .pact path"))?;
            let output = env::args().nth(4).unwrap_or_else(|| {
                let safe_range = range.replace("..", "-");
                format!("{source}.{safe_range}.slice.pact")
            });
            let spec = serde_json::json!({
                "mode": "transaction-range",
                "start_cycle": parsed_range.start,
                "end_cycle": parsed_range.end,
            });
            println!(
                "{}",
                file_format::file_slice_json(
                    Path::new(&source),
                    Path::new(&output),
                    &serde_json::to_string(&spec).map_err(|err| {
                        PactError::codec(format!("failed to encode generated slice spec: {err}"))
                    })?,
                )?
            );
            Ok(())
        }
        "file-slice" => {
            let source = env::args()
                .nth(2)
                .ok_or_else(|| PactError::invalid("file-slice requires a source .pact path"))?;
            let output = env::args()
                .nth(3)
                .ok_or_else(|| PactError::invalid("file-slice requires an output .pact path"))?;
            let spec = read_stdin()?;
            println!(
                "{}",
                file_format::file_slice_json(Path::new(&source), Path::new(&output), &spec)?
            );
            Ok(())
        }
        "file-import-check" => {
            let path = env::args()
                .nth(2)
                .ok_or_else(|| PactError::invalid("file-import-check requires a .pact path"))?;
            println!("{}", file_format::file_import_check_json(Path::new(&path))?);
            Ok(())
        }
        "import" | "file-import" => {
            let target = env::args()
                .nth(2)
                .ok_or_else(|| PactError::invalid("import requires a target .pact path"))?;
            let source = env::args()
                .nth(3)
                .ok_or_else(|| PactError::invalid("import requires a source slice .pact path"))?;
            println!(
                "{}",
                file_format::file_import_json(Path::new(&target), Path::new(&source))?
            );
            Ok(())
        }
        "merge" | "file-merge" => {
            let target = env::args()
                .nth(2)
                .ok_or_else(|| PactError::invalid("merge requires a target .pact path"))?;
            let source = env::args()
                .nth(3)
                .ok_or_else(|| PactError::invalid("merge requires a source slice .pact path"))?;
            println!(
                "{}",
                file_format::file_merge_json(Path::new(&target), Path::new(&source))?
            );
            Ok(())
        }
        "help" | "--help" | "-h" => {
            print_help();
            Ok(())
        }
        other => Err(PactError::invalid(format!(
            "unknown command {other:?}; run `pact help`"
        ))),
    }
}

fn read_node() -> PactResult<Component> {
    let input = read_stdin()?;
    serde_json::from_str(&input)
        .map_err(|err| PactError::invalid(format!("invalid PACT JSON: {err}")))
}

fn resolve_pact_target(target: &str) -> PactResult<PathBuf> {
    let direct = PathBuf::from(target);
    for candidate in pact_target_candidates(&direct) {
        if candidate.is_file() {
            return Ok(candidate);
        }
    }
    Err(PactError::invalid(format!(
        "could not resolve pact target: {target}"
    )))
}

fn pact_target_candidates(direct: &Path) -> Vec<PathBuf> {
    let mut candidates = Vec::new();
    candidates.push(direct.to_path_buf());
    if direct.is_dir() {
        candidates.push(direct.join("pact").join("current.pact"));
        candidates.push(direct.join("pact").join("session.pact"));
        candidates.push(direct.join("current.pact"));
        candidates.push(direct.join("session.pact"));
    }
    candidates
}

fn run_view_command() -> PactResult<()> {
    let subcommand = env::args().nth(2).ok_or_else(|| {
        PactError::invalid(
            "branch-view requires a subcommand: append|preflight|merge|detach|rebase".to_string(),
        )
    })?;
    match subcommand.as_str() {
        "append" => {
            let path = env::args()
                .nth(3)
                .ok_or_else(|| PactError::invalid("view append requires a .pact path"))?;
            let input = read_stdin()?;
            println!(
                "{}",
                file_format::file_append_view_records_json(
                    Path::new(&path),
                    &input,
                    blob_inline_limit(),
                )?
            );
            Ok(())
        }
        "preflight" => {
            let path = env::args()
                .nth(3)
                .ok_or_else(|| PactError::invalid("view preflight requires a .pact path"))?;
            let view_id = env::args()
                .nth(4)
                .ok_or_else(|| PactError::invalid("view preflight requires a view id"))?;
            println!(
                "{}",
                file_format::file_view_merge_preflight_json(Path::new(&path), &view_id)?
            );
            Ok(())
        }
        "merge" => {
            let path = env::args()
                .nth(3)
                .ok_or_else(|| PactError::invalid("view merge requires a .pact path"))?;
            let view_id = env::args()
                .nth(4)
                .ok_or_else(|| PactError::invalid("view merge requires a view id"))?;
            let policy = env::args().nth(5);
            println!(
                "{}",
                file_format::file_view_merge_json(Path::new(&path), &view_id, policy.as_deref())?
            );
            Ok(())
        }
        "detach" | "import-detached" => {
            let path = env::args()
                .nth(3)
                .ok_or_else(|| PactError::invalid("view detach requires a .pact path"))?;
            let view_id = env::args()
                .nth(4)
                .ok_or_else(|| PactError::invalid("view detach requires a view id"))?;
            let branch_id = env::args()
                .nth(5)
                .ok_or_else(|| PactError::invalid("view detach requires a detached branch id"))?;
            println!(
                "{}",
                file_format::file_view_detach_json(Path::new(&path), &view_id, &branch_id)?
            );
            Ok(())
        }
        "rebase" => {
            let path = env::args()
                .nth(3)
                .ok_or_else(|| PactError::invalid("view rebase requires a .pact path"))?;
            let view_id = env::args()
                .nth(4)
                .ok_or_else(|| PactError::invalid("view rebase requires a view id"))?;
            println!(
                "{}",
                file_format::file_view_rebase_json(
                    Path::new(&path),
                    &view_id,
                    blob_inline_limit()
                )?
            );
            Ok(())
        }
        other => Err(PactError::invalid(format!(
            "unknown branch-view subcommand {other:?}; expected append|preflight|merge|detach|rebase"
        ))),
    }
}

fn run_view_presentation_command() -> PactResult<()> {
    let target = env::args()
        .nth(2)
        .ok_or_else(|| PactError::invalid("view requires a target: head|archived"))?;
    let args = parse_view_args()?;
    let path = resolve_pact_target(&args.path)?;
    let branch = args.branch_id.as_deref();
    match target.as_str() {
        "head" => match args.format.as_str() {
            "json" => {
                println!(
                    "{}",
                    file_format::view_head_json(&path, branch, args.revision)?
                );
                Ok(())
            }
            "xml" => {
                print!(
                    "{}",
                    file_format::view_head_xml(&path, branch, args.revision)?
                );
                Ok(())
            }
            "rows" => {
                println!(
                    "{}",
                    file_format::view_head_rows_json(
                        &path,
                        branch,
                        args.revision,
                        blob_inline_limit()
                    )?
                );
                Ok(())
            }
            other => Err(PactError::invalid(format!(
                "unknown view format {other:?}; expected json|xml|rows"
            ))),
        },
        "archived" => match args.format.as_str() {
            "json" => {
                println!(
                    "{}",
                    file_format::view_archived_json(
                        &path,
                        branch,
                        args.revision,
                        args.content_limit()
                    )?
                );
                Ok(())
            }
            "rows" => {
                println!(
                    "{}",
                    file_format::view_archived_rows_json(
                        &path,
                        branch,
                        args.revision,
                        args.content_limit()
                    )?
                );
                Ok(())
            }
            other => Err(PactError::invalid(format!(
                "unknown view format {other:?}; expected json|rows"
            ))),
        },
        "projection" | "render" | "activity" | "all" => Err(PactError::invalid(format!(
            "{target:?} is not a public view target; use `view head` or `view archived`"
        ))),
        other => Err(PactError::invalid(format!(
            "unknown view target {other:?}; expected head or archived"
        ))),
    }
}

fn run_load_command() -> PactResult<()> {
    let subcommand = env::args().nth(2).ok_or_else(|| {
        PactError::invalid("load requires a .pact path or a subcommand: content-ref|components")
    })?;

    if subcommand.ends_with(".pact") || Path::new(&subcommand).is_file() {
        return run_load_file_command(&subcommand);
    }
    match subcommand.as_str() {
        "content-ref" => {
            let path = env::args()
                .nth(3)
                .ok_or_else(|| PactError::invalid("load content-ref requires a .pact path"))?;
            let content_ref = env::args()
                .nth(4)
                .ok_or_else(|| PactError::invalid("load content-ref requires a content ref id"))?;
            println!(
                "{}",
                file_format::file_content_payload_json(Path::new(&path), &content_ref)?
            );
            Ok(())
        }

        "components" => {
            let path = env::args()
                .nth(3)
                .ok_or_else(|| PactError::invalid("load components requires a .pact path"))?;
            let component_ids = env::args().skip(4).collect::<Vec<_>>();
            if component_ids.is_empty() {
                return Err(PactError::invalid(
                    "load components requires at least one component id".to_string(),
                ));
            }
            println!(
                "{}",
                file_format::file_hydrate_components_json(
                    Path::new(&path),
                    &component_ids,
                    None,
                    None
                )?
            );
            Ok(())
        }
        other => Err(PactError::invalid(format!(
            "unknown load subcommand {other:?}; expected content-ref or components"
        ))),
    }
}

fn run_load_file_command(path: &str) -> PactResult<()> {
    let mut branch_id = None;
    let mut revision = None;
    let mut parent_paths: Vec<PathBuf> = Vec::new();
    let mut args = env::args().skip(3);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--parent-path" => {
                parent_paths.push(PathBuf::from(args.next().ok_or_else(|| {
                    PactError::invalid("--parent-path requires a directory")
                })?));
            }
            "--branch" => {
                branch_id = Some(
                    args.next()
                        .ok_or_else(|| PactError::invalid("--branch requires a branch id"))?,
                );
            }
            "--revision" => {
                revision = Some(
                    args.next()
                        .ok_or_else(|| PactError::invalid("--revision requires a number"))?
                        .parse::<u64>()
                        .map_err(|_| PactError::invalid("invalid --revision number"))?,
                );
            }
            "--json" => {}
            other => {
                return Err(PactError::invalid(format!(
                    "unknown load argument: {other}"
                )));
            }
        }
    }

    let child_path = Path::new(path);
    let parent_config = parent_resolver_config_from_paths(parent_paths).unwrap_or_default();
    println!(
        "{}",
        file_format::file_load_child_head_json(
            child_path,
            branch_id.as_deref(),
            revision,
            &parent_config,
        )?
    );
    Ok(())
}

fn run_fork_command() -> PactResult<()> {
    let mut positionals: Vec<String> = Vec::new();
    let mut mode = None;
    let mut base = None;
    let mut branch_id = None;
    let mut revision = None;
    let mut locator_hints = None;
    let mut args = env::args().skip(2);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--mode" => {
                mode =
                    Some(args.next().ok_or_else(|| {
                        PactError::invalid("--mode requires hard, soft, or delta")
                    })?);
            }
            "--base" => {
                base =
                    Some(args.next().ok_or_else(|| {
                        PactError::invalid("--base requires pinned or following")
                    })?);
            }
            "--branch" => {
                branch_id = Some(
                    args.next()
                        .ok_or_else(|| PactError::invalid("--branch requires a branch id"))?,
                );
            }
            "--revision" => {
                revision = Some(
                    args.next()
                        .ok_or_else(|| PactError::invalid("--revision requires a number"))?
                        .parse::<u64>()
                        .map_err(|_| PactError::invalid("invalid --revision number"))?,
                );
            }
            "--locator-hints" => {
                locator_hints =
                    Some(args.next().ok_or_else(|| {
                        PactError::invalid("--locator-hints requires a JSON array")
                    })?);
            }
            "--json" => {}
            value if value.starts_with("--") => {
                return Err(PactError::invalid(format!(
                    "unknown fork argument: {value}"
                )));
            }
            value => positionals.push(value.to_string()),
        }
    }

    let mode = match mode.as_deref() {
        Some("hard") => file_format::PactForkMode::Hard,
        Some("soft") => file_format::PactForkMode::Soft,
        Some("delta") => {
            let base_pinned = match base.as_deref() {
                Some("pinned") | None => true,
                Some("following") => false,
                Some(other) => {
                    return Err(PactError::invalid(format!(
                        "unknown --base {other:?}; expected pinned or following"
                    )));
                }
            };
            file_format::PactForkMode::Delta { base_pinned }
        }
        Some(other) => {
            return Err(PactError::invalid(format!(
                "unknown fork mode {other:?}; expected hard, soft, or delta"
            )));
        }
        None => {
            return Err(PactError::invalid(
                "fork requires --mode hard, --mode soft, or --mode delta",
            ));
        }
    };
    if base.is_some() && !matches!(mode, file_format::PactForkMode::Delta { .. }) {
        return Err(PactError::invalid("--base is only valid with --mode delta"));
    }

    let source = positionals
        .first()
        .ok_or_else(|| PactError::invalid("fork requires a parent .pact path"))?;
    let output = positionals.get(1);
    if matches!(
        mode,
        file_format::PactForkMode::Hard | file_format::PactForkMode::Delta { .. }
    ) && output.is_none()
    {
        let mode_name = if matches!(mode, file_format::PactForkMode::Hard) {
            "hard"
        } else {
            "delta"
        };
        return Err(PactError::invalid(format!(
            "{mode_name} fork requires a child .pact output path: pact fork parent.pact child.pact --mode {mode_name}"
        )));
    }

    println!(
        "{}",
        file_format::file_fork_json(
            Path::new(source),
            output.map(Path::new),
            mode,
            branch_id.as_deref(),
            revision,
            locator_hints.as_deref(),
            blob_inline_limit(),
        )?
    );
    Ok(())
}

fn run_send_message_command() -> PactResult<()> {
    let mut from_path = None;
    let mut to_path = None;
    let mut text = None;
    let mut to_branch = None;
    let mut source_trace_id = None;
    let mut source_tool_call_id = None;
    let mut created_at = None;
    let mut args = env::args().skip(2);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--from" => {
                from_path = Some(
                    args.next()
                        .ok_or_else(|| PactError::invalid("--from requires a .pact path"))?,
                );
            }
            "--to" => {
                to_path = Some(
                    args.next()
                        .ok_or_else(|| PactError::invalid("--to requires a .pact path"))?,
                );
            }
            "--text" => {
                text = Some(
                    args.next()
                        .ok_or_else(|| PactError::invalid("--text requires a string"))?,
                );
            }
            "--to-branch" => {
                to_branch = Some(
                    args.next()
                        .ok_or_else(|| PactError::invalid("--to-branch requires a branch id"))?,
                );
            }
            "--source-trace-id" => {
                source_trace_id = Some(
                    args.next()
                        .ok_or_else(|| PactError::invalid("--source-trace-id requires an id"))?,
                );
            }
            "--source-tool-call-id" => {
                source_tool_call_id =
                    Some(args.next().ok_or_else(|| {
                        PactError::invalid("--source-tool-call-id requires an id")
                    })?);
            }
            "--created-at" => {
                created_at = Some(
                    args.next()
                        .ok_or_else(|| PactError::invalid("--created-at requires a timestamp"))?,
                );
            }
            "--json" => {}
            value if value.starts_with("--") => {
                return Err(PactError::invalid(format!(
                    "unknown send-message argument: {value}"
                )));
            }
            value => {
                return Err(PactError::invalid(format!(
                    "unexpected send-message positional: {value}"
                )));
            }
        }
    }

    let from_path =
        from_path.ok_or_else(|| PactError::invalid("send-message requires --from <a.pact>"))?;
    let to_path =
        to_path.ok_or_else(|| PactError::invalid("send-message requires --to <b.pact>"))?;
    let text = text.ok_or_else(|| PactError::invalid("send-message requires --text <text>"))?;

    let from_root_id = file_format::file_root_id(Path::new(&from_path))?;
    let to_root_id = file_format::file_root_id(Path::new(&to_path))?;

    let report = pactree::send_context_message(pactree::CrossContextMessage {
        from_context: pactree::PactContextRef {
            root_id: from_root_id,
            branch_id: None,
            head_ref: None,
            path: Some(PathBuf::from(&from_path)),
        },
        to_context: pactree::PactContextRef {
            root_id: to_root_id,
            branch_id: to_branch,
            head_ref: None,
            path: Some(PathBuf::from(&to_path)),
        },
        parts: vec![pactree::text_message_part(&text)?],
        source_trace_id,
        source_tool_call_id,
        created_at,
        aux: std::collections::BTreeMap::new(),
    })?;

    let report_json = serde_json::json!({
        "schema": "pact.cross_context_message.delivery.v1",
        "from": from_path,
        "to": to_path,
        "delivered_component_id": report.delivered_component_id,
        "delivery_trace_id": report.delivery_trace_id,
        "from_root_id": report.from_root_id,
        "to_root_id": report.to_root_id,
        "source_trace_id": report.source_trace_id,
        "source_tool_call_id": report.source_tool_call_id,
        "to_branch_id": report.to_branch_id,
        "to_revision": report.to_revision,
    });
    println!(
        "{}",
        serde_json::to_string_pretty(&report_json).map_err(|err| PactError::codec(format!(
            "failed to encode send-message report: {err}"
        )))?
    );
    Ok(())
}

#[derive(Default)]
struct ViewArgs {
    path: String,
    format: String,
    content: String,
    branch_id: Option<String>,
    revision: Option<u64>,
}

impl ViewArgs {
    fn content_limit(&self) -> usize {
        if self.content == "full" {
            blob_inline_limit()
        } else {
            0
        }
    }
}

fn parse_view_args() -> PactResult<ViewArgs> {
    let mut parsed = ViewArgs {
        format: "json".to_string(),
        content: "refs".to_string(),
        ..ViewArgs::default()
    };
    let mut args = env::args().skip(3);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--branch" => {
                parsed.branch_id = Some(
                    args.next()
                        .ok_or_else(|| PactError::invalid("--branch requires a branch id"))?,
                );
            }
            "--revision" => {
                let value = args
                    .next()
                    .ok_or_else(|| PactError::invalid("--revision requires a revision"))?;
                parsed.revision = Some(value.parse::<u64>().map_err(|err| {
                    PactError::invalid(format!("invalid --revision value: {err}"))
                })?);
            }
            "--format" => {
                parsed.format = args
                    .next()
                    .ok_or_else(|| PactError::invalid("--format requires json|rows|xml"))?;
            }
            "--content" => {
                let value = args
                    .next()
                    .ok_or_else(|| PactError::invalid("--content requires refs|full"))?;
                match value.as_str() {
                    "refs" | "full" => parsed.content = value,
                    _ => {
                        return Err(PactError::invalid(format!(
                            "unknown --content mode: {value}; expected refs|full"
                        )));
                    }
                }
            }
            value if value.starts_with("--") => {
                return Err(PactError::invalid(format!(
                    "unknown view argument: {value}"
                )));
            }
            value => {
                if parsed.path.is_empty() {
                    parsed.path = value.to_string();
                } else {
                    return Err(PactError::invalid(format!(
                        "unexpected view argument: {value}"
                    )));
                }
            }
        }
    }
    if parsed.path.is_empty() {
        return Err(PactError::invalid(
            "view requires a .pact path or a directory containing a .pact file".to_string(),
        ));
    }
    Ok(parsed)
}

struct TextSearchArgs {
    path: String,
    query: file_format::PactTextSearchQuery,
}

struct FileSelectArgs {
    path: String,
    query: file_format::PactFileSelectorQuery,

    parent_config: Option<file_format::ParentResolverConfig>,
}

fn parse_file_select_args() -> PactResult<FileSelectArgs> {
    parse_file_select_args_from(env::args().skip(2))
}

fn parse_file_select_args_from<I>(raw_args: I) -> PactResult<FileSelectArgs>
where
    I: IntoIterator<Item = String>,
{
    let mut path = None;
    let mut selector = None;
    let mut parent_paths: Vec<PathBuf> = Vec::new();
    let mut query = file_format::PactFileSelectorQuery::new(String::new());
    let mut args = raw_args.into_iter();
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--limit" => {
                query.limit = args
                    .next()
                    .ok_or_else(|| PactError::invalid("--limit requires a count"))?
                    .parse::<usize>()
                    .map_err(|_| PactError::invalid("invalid --limit count"))?;
            }
            "--order" => {
                query.order = args
                    .next()
                    .ok_or_else(|| PactError::invalid("--order requires newest or oldest"))?;
            }
            "--branch" => {
                query.branch_id = Some(
                    args.next()
                        .ok_or_else(|| PactError::invalid("--branch requires a branch id"))?,
                );
            }
            "--parent-path" => {
                parent_paths.push(PathBuf::from(args.next().ok_or_else(|| {
                    PactError::invalid("--parent-path requires a directory")
                })?));
            }
            "--json" => {}
            value if value.starts_with("--") => {
                return Err(PactError::invalid(format!(
                    "unknown select argument: {value}"
                )));
            }
            value if path.is_none() => path = Some(value.to_string()),
            value if selector.is_none() => selector = Some(value.to_string()),
            value => {
                return Err(PactError::invalid(format!(
                    "unexpected select argument: {value}"
                )));
            }
        }
    }
    query.selector =
        selector.ok_or_else(|| PactError::invalid("select requires a PACT selector"))?;
    let parent_config = parent_resolver_config_from_paths(parent_paths);
    Ok(FileSelectArgs {
        path: path.ok_or_else(|| PactError::invalid("select requires a .pact path"))?,
        query,
        parent_config,
    })
}

fn parent_resolver_config_from_paths(
    parent_paths: Vec<PathBuf>,
) -> Option<file_format::ParentResolverConfig> {
    if parent_paths.is_empty() {
        return None;
    }
    Some(file_format::ParentResolverConfig {
        base_dir: parent_paths.first().cloned(),
        search_paths: parent_paths,
        explicit_parents: std::collections::BTreeMap::new(),
    })
}

fn parse_text_search_args_from<I>(raw_args: I) -> PactResult<TextSearchArgs>
where
    I: IntoIterator<Item = String>,
{
    let mut path = None;
    let mut needle = None;
    let mut query = file_format::PactTextSearchQuery::new("");
    let mut args = raw_args.into_iter();
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--scope" => {
                query.scope = args
                    .next()
                    .ok_or_else(|| PactError::invalid("--scope requires all or head"))?;
            }
            "--stream" => {
                return Err(PactError::invalid(
                    "search text uses --scope all|head; --stream is not supported".to_string(),
                ));
            }
            "--select" => {
                query.selector = Some(
                    args.next()
                        .ok_or_else(|| PactError::invalid("--select requires a selector"))?,
                );
            }
            "--case-insensitive" | "-i" => query.case_insensitive = true,
            "--regex" => query.regex = true,
            "--limit" => {
                query.limit = args
                    .next()
                    .ok_or_else(|| PactError::invalid("--limit requires a count"))?
                    .parse::<usize>()
                    .map_err(|_| PactError::invalid("invalid --limit count"))?;
            }
            "--preview-chars" => {
                query.preview_chars = args
                    .next()
                    .ok_or_else(|| PactError::invalid("--preview-chars requires a count"))?
                    .parse::<usize>()
                    .map_err(|_| PactError::invalid("invalid --preview-chars count"))?;
            }
            "--json" => {}
            value if value.starts_with("--") => {
                return Err(PactError::invalid(format!(
                    "unknown search text argument: {value}"
                )));
            }
            value if path.is_none() => path = Some(value.to_string()),
            value if needle.is_none() => needle = Some(value.to_string()),
            value => {
                return Err(PactError::invalid(format!(
                    "unexpected search text argument: {value}"
                )));
            }
        }
    }
    query.text =
        needle.ok_or_else(|| PactError::invalid("search text requires a search string"))?;
    Ok(TextSearchArgs {
        path: path.ok_or_else(|| PactError::invalid("search text requires a .pact path"))?,
        query,
    })
}

fn run_search_command() -> PactResult<()> {
    let mut args = env::args().skip(2);
    let kind = args
        .next()
        .ok_or_else(|| PactError::invalid("search requires text or vector"))?;
    match kind.as_str() {
        "text" | "grep" => {
            let args = parse_text_search_args_from(args)?;
            println!(
                "{}",
                file_format::file_text_search_json(Path::new(&args.path), args.query)?
            );
            Ok(())
        }
        "vector" => {
            let mut extension_args = args.collect::<Vec<_>>();
            let subcommand = extension_args.first().map(String::as_str);
            if !matches!(
                subcommand,
                Some("status" | "rebuild" | "search" | "inspect" | "clear")
            ) {
                extension_args.insert(0, "search".to_string());
            }
            run_vector_search_extension(&extension_args)
        }
        other => Err(PactError::invalid(format!(
            "unsupported search kind {other:?}; expected text or vector"
        ))),
    }
}

fn run_vector_search_extension(args: &[String]) -> PactResult<()> {
    let bin = env::var_os("PACT_VECTOR_SEARCH_BIN")
        .map(PathBuf::from)
        .or_else(|| installed_extension_command("vector-search"))
        .or_else(|| installed_extension_command("pact-vector-search"))
        .unwrap_or_else(|| PathBuf::from("pact-vector-search"));
    let status = Command::new(&bin)
        .args(args)
        .stdin(Stdio::inherit())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .status()
        .map_err(|err| { PactError::invalid(format!(
                "failed to run vector search extension {}: {err}. Install it with `pact-vector-search install-extension`, `pactree extension install vector-search /path/to/pact-vector-search`, or set PACT_VECTOR_SEARCH_BIN.",
                bin.display()
            )) })?;
    if status.success() {
        Ok(())
    } else {
        Err(PactError::invalid(format!(
            "vector search extension exited with status {}",
            status
        )))
    }
}

fn run_extension_command() -> PactResult<()> {
    let mut args = env::args().skip(2);
    let subcommand = args
        .next()
        .ok_or_else(|| PactError::invalid("extension requires install, list, or remove"))?;
    match subcommand.as_str() {
        "install" => {
            let name = args
                .next()
                .ok_or_else(|| PactError::invalid("extension install requires a name"))?;
            let command = args
                .next()
                .ok_or_else(|| PactError::invalid("extension install requires a command path"))?;
            install_extension(&name, Path::new(&command))?;
            println!(
                "{}",
                serde_json::to_string_pretty(&serde_json::json!({
                    "schema": "pactree.extension.install.v1",
                    "name": name,
                    "command": command,
                    "manifest": extension_manifest_path(&name)?,
                }))
                .map_err(|err| PactError::codec(err.to_string()))?
            );
            Ok(())
        }
        "list" => {
            let dir = extension_dir()?;
            let mut entries = Vec::new();
            if dir.exists() {
                for entry in fs::read_dir(&dir).map_err(PactError::from)? {
                    let entry = entry.map_err(PactError::from)?;
                    if entry.path().extension().and_then(|ext| ext.to_str()) == Some("json") {
                        let raw = fs::read_to_string(entry.path()).map_err(PactError::from)?;
                        if let Ok(value) = serde_json::from_str::<serde_json::Value>(&raw) {
                            entries.push(value);
                        }
                    }
                }
            }
            println!(
                "{}",
                serde_json::to_string_pretty(&serde_json::json!({
                    "schema": "pactree.extensions.v1",
                    "directory": dir,
                    "extensions": entries,
                }))
                .map_err(|err| PactError::codec(err.to_string()))?
            );
            Ok(())
        }
        "remove" | "uninstall" => {
            let name = args
                .next()
                .ok_or_else(|| PactError::invalid("extension remove requires a name"))?;
            let path = extension_manifest_path(&name)?;
            if path.exists() {
                fs::remove_file(&path).map_err(PactError::from)?;
            }
            println!(
                "{}",
                serde_json::to_string_pretty(&serde_json::json!({
                    "schema": "pactree.extension.remove.v1",
                    "name": name,
                    "removed": true,
                }))
                .map_err(|err| PactError::codec(err.to_string()))?
            );
            Ok(())
        }
        other => Err(PactError::invalid(format!(
            "unsupported extension command {other:?}; expected install, list, or remove"
        ))),
    }
}

fn install_extension(name: &str, command: &Path) -> PactResult<()> {
    let path = extension_manifest_path(name)?;
    let dir = path
        .parent()
        .ok_or_else(|| PactError::invalid("extension manifest path has no parent"))?;
    fs::create_dir_all(dir).map_err(PactError::from)?;
    let command = command
        .canonicalize()
        .unwrap_or_else(|_| command.to_path_buf());
    let manifest = serde_json::json!({
        "schema": "pactree.extension.v1",
        "name": name,
        "command": command,
    });
    let bytes =
        serde_json::to_vec_pretty(&manifest).map_err(|err| PactError::codec(err.to_string()))?;
    publish_bytes(&path, &bytes, PublishMode::ReplaceExisting)
}

fn installed_extension_command(name: &str) -> Option<PathBuf> {
    let path = extension_manifest_path(name).ok()?;
    let raw = fs::read_to_string(path).ok()?;
    let value = serde_json::from_str::<serde_json::Value>(&raw).ok()?;
    value
        .get("command")
        .and_then(serde_json::Value::as_str)
        .map(PathBuf::from)
}

fn extension_manifest_path(name: &str) -> PactResult<PathBuf> {
    if !name
        .chars()
        .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.'))
    {
        return Err(PactError::invalid(format!(
            "invalid extension name: {name}"
        )));
    }
    Ok(extension_dir()?.join(format!("{name}.json")))
}

fn extension_dir() -> PactResult<PathBuf> {
    if let Some(dir) = env::var_os("PACTREE_EXTENSION_DIR") {
        return Ok(PathBuf::from(dir));
    }
    if let Some(dir) = env::var_os("XDG_CONFIG_HOME") {
        return Ok(PathBuf::from(dir).join("pactree").join("extensions"));
    }
    let home = env::var_os("HOME").ok_or_else(|| {
        PactError::invalid(
            "HOME is not set; set PACTREE_EXTENSION_DIR to install Pactree extensions".to_string(),
        )
    })?;
    Ok(PathBuf::from(home)
        .join(".config")
        .join("pactree")
        .join("extensions"))
}

fn cli_prune_ledger_json(
    root_store_json: &str,
    branch_id: &str,
    selector: &str,
) -> PactResult<String> {
    pactree::ledger_delete_json(root_store_json, branch_id, None, Some(selector))
}

fn read_stdin() -> PactResult<String> {
    let mut input = String::new();
    io::stdin()
        .read_to_string(&mut input)
        .map_err(|err| PactError::storage(format!("failed to read stdin: {err}")))?;
    Ok(input)
}

fn blob_inline_limit() -> usize {
    env::var("PACT_BLOB_INLINE_LIMIT")
        .ok()
        .and_then(|value| value.parse::<usize>().ok())
        .unwrap_or(1024)
}

fn payload_inline_limit_for_profile(profile: &str) -> PactResult<usize> {
    match profile {
        "hot" => Ok(blob_inline_limit()),
        "archive" => Ok(0),
        "debug" => Ok(usize::MAX / 2),
        other => Err(PactError::invalid(format!(
            "unknown payload profile {other:?}; expected hot, archive, or debug"
        ))),
    }
}

fn print_help() {
    let help = "pactree

USAGE:
  pact normalize < pact.json
  pact inspect < pact.json
  pact select 'd0 > .text_content' < pact.json
  pact select session.pact '@history .text_content' [--limit n] [--order newest|oldest] [--branch id] [--parent-path dir] [--json]
  pact prune '#tool_call_component_id' [branch_id] < root-store.json
  pact store-new < pact.json
  pact root-new [--no-base] [branch_id] < pact.json
  pact root-from-store [branch_id] < store.json
  pact root-branch-revision <from_branch> [revision] [new_branch] < root-store.json   # same-ledger branch, NOT a hard fork
  pact root-branch-component <from_branch> <component_id> [new_branch] < root-store.json   # same-ledger branch, NOT a hard fork
  pact store-project [revision] < store.json
  pact content-put provider.request.rows < payload.txt
  pact transaction-validate < trace.json
  pact file-create [--token-span|--no-token-span] session.pact < root-store.json
  pact file-append-views session.pact < view-records.json
  pact inspect timeline session.pact
  pact file-inspect [--full|--records] session.pact
  pact file-order-check session.pact
  pact file-order-repair session.pact --i-have-a-copy [--branch id]   # derived index only; copies only
  pact token-span-stats session.pact
  pact page-compare baseline.pact candidate.pact
  pact info session.pact
  pact info --manifest-only session.pact
  pact info --live-shape session.pact
  pact storage-audit session.pact
  pact bench-storage [--target-profile medium-agentic] session.pact
  pact file-validate session.pact
  pact check session.pact
  pact doctor session.pact
  pact fork parent.pact child.pact --mode hard [--branch id] [--revision n] [--locator-hints '[{\"kind\":\"relative_path\",\"value\":\"parent.pact\"}]']
  pact fork parent.pact --mode soft [--branch id] [--revision n]
  pact fork parent.pact child.pact --mode delta --base pinned [--branch id] [--revision n] [--locator-hints '[...]']
  pact fork parent.pact child.pact --mode delta --base following [--branch id] [--revision n]
  pact send-message --from a.pact --to b.pact --text \"msg\" [--to-branch id] [--source-trace-id id] [--source-tool-call-id id] [--created-at ts]
  pact file-rewind-to session.pact [branch_id] <revision>
  pact file-repack source.pact output.pact [--token-span|--no-token-span] [--format v0|packed-v1]
  pact file-reanchor source.pact output.pact [--freeze-cycle-interval n|--every-cycles n] [--freeze-commit-bytes n|--max-bytes n] [--no-anchors]
  pact reindex-head source.pact [output.pact|--output output.pact]
  pact no-anchor source.pact output.pact
  pact bundle create session.pact
  pact bundle verify session.pact
  pact bundle export-full session.pact session.pact.full
  pact bundle prune session.pact
  pact view head session.pact [--format json|rows|xml] [--branch id] [--revision n]
  pact view archived session.pact [--format json|rows] [--branch id] [--revision n] [--content refs|full]
  pact load child.pact [--parent-path dir] [--branch id] [--revision n]
  pact load content-ref session.pact <content_ref>
  pact load components session.pact <component_id> [<component_id> ...]
  pact branch-view append session.pact < view-records.json
  pact branch-view preflight session.pact <view_id>
  pact branch-view merge session.pact <view_id> [replace-component|manual-resolution]
  pact branch-view detach session.pact <view_id> <detached_branch_id>
  pact branch-view rebase session.pact <view_id>
  pact extension install <name> <command-path>
  pact extension list
  pact extension remove <name>
  pact file-repair source.pact repaired.pact [--truncate-tail]
  pact heal source.pact --out healed.pact [--truncate-tail]
  pact search text session.pact \"needle\" [--scope all|head] [--select selector] [--case-insensitive] [--regex] [--limit n] [--preview-chars n] [--json]
  pact search vector session.pact \"query\" [--select selector] [--provider qwen|hash-dev] [--model model] [--top-k n] [--json]
  pact search vector rebuild session.pact [--provider qwen|hash-dev] [--model model] [--scope all|head] [--json]
  pact search vector status|inspect|clear session.pact [...]
  pact slice cycle_start..cycle_end source.pact [output.pact]
  pact file-slice source.pact output.pact < slice-spec.json
  pact file-import-check session.pact
  pact import target.pact slice.pact
  pact merge target.pact slice.pact

INPUT:
  A JSON PACT component with id, componentType, optional type, offset,
  priority, transaction metadata, created_at_ns, creation_index, aux,
  content, children.

SELECTORS:
  Rust PACT selectors are dN-first: d-1, d0, d1, (d0,0), .type, #id,
  {attr='value'}, >, ?, ??, and comma unions. Caret aliases are deprecated.

DELETE:
  Deleting any tool call/result component automatically deletes its paired
  component with the same tool_call_id.

TRANSACTIONS:
  Transaction traces capture authored, pre-request, post-request-alteration,
  provider-request, provider-response, post-response-alteration, and
  post-response refs. Tool dispatch is not a boundary.

COMPONENTS:
  Component records and component transactions are generic durable extension
  records. PACT owns ids, ordering, refs, indexes, repair, and slices; extension
  packages own domain semantics.

READING:
  pactree view head / pactree view archived present PACT state (--format json|rows|xml,
  --content refs|full). Head is the current canonical state; archived is retained
  historical state. pactree load resolves content (content-ref, components); pactree select
  queries components; pactree search text|vector finds content; pactree inspect timeline
  shows trace/event activity.

BRANCH VIEWS:
  pactree branch-view manages branch perspectives (append/preflight/merge/detach/
  rebase). Merge preflight is non-mutating; merge applies explicit component
  replacement; detach imports a view branch as a separate branch.

PACT FILES:
  file-create writes the binary-framed .pact file: pages, typed records,
  symbols, content chunks, structural/component records, query indexes,
  explicit repair, native view data, and portable slices.";
    println!("{}", help.replace("\n  pact ", "\n  pactree "));
}
