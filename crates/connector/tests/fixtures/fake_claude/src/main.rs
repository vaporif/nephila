//! Test fixture imitating `claude --print --verbose --input-format stream-json --output-format stream-json`.
//!
//! Hand-rolled flag parsing (no `clap`) keeps dev-deps minimal. The fake honours
//! `--session-id` / `--resume` so resume/session-id stability tests in later
//! slices are not silently using a placeholder.

use std::io::{BufRead, Write};

#[derive(Default)]
struct Args {
    session_id: Option<String>,
    resume: Option<String>,
    scenario: Scenario,
}

#[derive(Default, Clone, Copy)]
enum Scenario {
    #[default]
    Happy,
    CrashMidTurn,
    Malformed,
    OversizedToolResult,
    SlowWriter,
    SlowReader,
    /// On each stdin line, emit one text frame, sleep 100ms, repeat — used by
    /// pause/resume tests in slice 3 to verify SIGSTOP halts emissions.
    SteadyDrip,
}

fn parse_args() -> Args {
    let mut a = Args::default();
    let mut it = std::env::args().skip(1);
    while let Some(flag) = it.next() {
        match flag.as_str() {
            "--session-id" => a.session_id = it.next(),
            "--resume" => a.resume = it.next(),
            "--scenario" => {
                a.scenario = match it.next().as_deref() {
                    Some("happy") | None => Scenario::Happy,
                    Some("crash_mid_turn") => Scenario::CrashMidTurn,
                    Some("malformed") => Scenario::Malformed,
                    Some("oversized_tool_result") => Scenario::OversizedToolResult,
                    Some("slow_writer") => Scenario::SlowWriter,
                    Some("slow_reader") => Scenario::SlowReader,
                    Some("steady_drip") => Scenario::SteadyDrip,
                    Some(other) => {
                        eprintln!("unknown scenario: {other}");
                        std::process::exit(2);
                    }
                };
            }
            // Real-claude flags we accept but ignore (to keep the spawn flag set in lockstep):
            "--print" | "--verbose" | "--include-partial-messages" => {}
            "--input-format" | "--output-format" | "--mcp-config" | "--permission-mode"
            | "--settings" => {
                let _ = it.next();
            }
            other => eprintln!("fake_claude: ignoring unknown flag: {other}"),
        }
    }
    a
}

fn main() -> std::io::Result<()> {
    let args = parse_args();
    let stdout = std::io::stdout();
    let mut out = stdout.lock();
    // `--resume` wins over `--session-id`, matching real claude's behaviour.
    let id = args
        .resume
        .as_deref()
        .or(args.session_id.as_deref())
        .unwrap_or("00000000-0000-0000-0000-000000000000");
    writeln!(
        out,
        "{{\"type\":\"system\",\"subtype\":\"init\",\"session_id\":\"{id}\"}}"
    )?;
    out.flush()?;

    // One stdin line drives one scripted response cycle; line content is ignored.
    for line in std::io::BufReader::new(std::io::stdin()).lines() {
        let _ = line?;
        match args.scenario {
            Scenario::Happy => emit_happy(&mut out, id)?,
            Scenario::CrashMidTurn => {
                emit_partial(&mut out, id)?;
                std::process::exit(137);
            }
            Scenario::Malformed => {
                writeln!(out, "{{not valid json")?;
                out.flush()?;
            }
            Scenario::OversizedToolResult => emit_oversized_tool_result(&mut out, id)?,
            Scenario::SlowWriter => {
                std::thread::sleep(std::time::Duration::from_millis(200));
                emit_happy(&mut out, id)?;
            }
            Scenario::SlowReader => emit_burst(&mut out, id, 200)?,
            Scenario::SteadyDrip => emit_steady_drip(&mut out, id)?,
        }
    }
    Ok(())
}

/// Emits ~10 text frames at 100ms intervals, then a result frame. Used by
/// pause/resume tests: pausing mid-burst yields a fixed gap; resuming
/// continues emission.
fn emit_steady_drip(out: &mut impl Write, session_id: &str) -> std::io::Result<()> {
    for i in 0..10 {
        writeln!(
            out,
            "{{\"type\":\"assistant\",\"message\":{{\"id\":\"msg-drip\",\"role\":\"assistant\",\"model\":\"fake\",\"content\":[{{\"type\":\"text\",\"text\":\"d{i}\"}}]}},\"session_id\":\"{session_id}\"}}"
        )?;
        out.flush()?;
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    writeln!(
        out,
        "{{\"type\":\"assistant\",\"message\":{{\"id\":\"msg-drip\",\"role\":\"assistant\",\"model\":\"fake\",\"content\":[{{\"type\":\"text\",\"text\":\"END\"}}],\"stop_reason\":\"end_turn\"}},\"session_id\":\"{session_id}\"}}"
    )?;
    writeln!(
        out,
        "{{\"type\":\"result\",\"subtype\":\"success\",\"is_error\":false,\"duration_ms\":1,\"duration_api_ms\":1,\"num_turns\":1,\"session_id\":\"{session_id}\",\"total_cost_usd\":0.0,\"stop_reason\":\"end_turn\"}}"
    )?;
    out.flush()
}

/// Canned happy frame sequence: two text deltas (same message id) then result.
fn emit_happy(out: &mut impl Write, session_id: &str) -> std::io::Result<()> {
    writeln!(
        out,
        "{{\"type\":\"assistant\",\"message\":{{\"id\":\"msg-1\",\"role\":\"assistant\",\"model\":\"fake\",\"content\":[{{\"type\":\"text\",\"text\":\"O\"}}]}},\"session_id\":\"{session_id}\"}}"
    )?;
    writeln!(
        out,
        "{{\"type\":\"assistant\",\"message\":{{\"id\":\"msg-1\",\"role\":\"assistant\",\"model\":\"fake\",\"content\":[{{\"type\":\"text\",\"text\":\"K\"}}],\"stop_reason\":\"end_turn\"}},\"session_id\":\"{session_id}\"}}"
    )?;
    writeln!(
        out,
        "{{\"type\":\"result\",\"subtype\":\"success\",\"is_error\":false,\"duration_ms\":1,\"duration_api_ms\":1,\"num_turns\":1,\"session_id\":\"{session_id}\",\"total_cost_usd\":0.0,\"stop_reason\":\"end_turn\"}}"
    )?;
    out.flush()
}

fn emit_partial(out: &mut impl Write, session_id: &str) -> std::io::Result<()> {
    writeln!(
        out,
        "{{\"type\":\"assistant\",\"message\":{{\"id\":\"msg-1\",\"role\":\"assistant\",\"model\":\"fake\",\"content\":[{{\"type\":\"text\",\"text\":\"partial\"}}]}},\"session_id\":\"{session_id}\"}}"
    )?;
    out.flush()
}

fn emit_oversized_tool_result(out: &mut impl Write, session_id: &str) -> std::io::Result<()> {
    let big = "x".repeat(300 * 1024);
    writeln!(
        out,
        "{{\"type\":\"assistant\",\"message\":{{\"id\":\"msg-1\",\"role\":\"assistant\",\"model\":\"fake\",\"content\":[{{\"type\":\"mcp_tool_result\",\"tool_use_id\":\"toolu_1\",\"content\":\"{big}\"}}]}},\"session_id\":\"{session_id}\"}}"
    )?;
    out.flush()
}

fn emit_burst(out: &mut impl Write, session_id: &str, n: usize) -> std::io::Result<()> {
    for i in 0..n {
        writeln!(
            out,
            "{{\"type\":\"assistant\",\"message\":{{\"id\":\"msg-{i}\",\"role\":\"assistant\",\"model\":\"fake\",\"content\":[{{\"type\":\"text\",\"text\":\"x\"}}]}},\"session_id\":\"{session_id}\"}}"
        )?;
    }
    writeln!(
        out,
        "{{\"type\":\"result\",\"subtype\":\"success\",\"is_error\":false,\"duration_ms\":1,\"duration_api_ms\":1,\"num_turns\":1,\"session_id\":\"{session_id}\",\"total_cost_usd\":0.0,\"stop_reason\":\"end_turn\"}}"
    )?;
    out.flush()
}
