//! `lictor`: agent, CLI and native HTTP API in one binary.

mod agent_cmd;
mod api;
mod cli;
mod client;
mod commands;
mod confdir;
mod output;
mod sys;
mod ui;

use std::io::Write;
use std::process::ExitCode;

fn init_logging(level: &str, format: &str) {
    use tracing_subscriber::EnvFilter;
    let filter = std::env::var("RUST_LOG")
        .ok()
        .and_then(|f| EnvFilter::try_new(f).ok())
        .or_else(|| EnvFilter::try_new(level).ok())
        .unwrap_or_else(|| EnvFilter::new("info"));
    let color = std::io::IsTerminal::is_terminal(&std::io::stderr()) && std::env::var_os("NO_COLOR").is_none();
    let builder = tracing_subscriber::fmt().with_env_filter(filter).with_writer(std::io::stderr).with_ansi(color);
    if format == "json" {
        builder.json().init();
    } else {
        builder.with_target(false).init();
    }
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if matches!(args.as_slice(), [a] if a == "--version" || a == "-V") {
        println!("lictor {}", env!("CARGO_PKG_VERSION"));
        return ExitCode::SUCCESS;
    }

    let mut cli = cli::build();
    let values = cli.run_with(&args);
    let path = cli.command_path();

    let is_agent = path.first().is_some_and(|p| p == "agent");
    // Commands log only warnings unless asked; the agent logs at the configured level.
    let level = if is_agent || cli.is_explicitly_set("log-level") { values.str("LICTOR_LOG_LEVEL") } else { "warn" };
    init_logging(level, values.str("LICTOR_LOG_FORMAT"));

    let runtime = match tokio::runtime::Builder::new_multi_thread().enable_all().build() {
        Ok(rt) => rt,
        Err(e) => {
            eprintln!("Error: cannot start the async runtime: {e}");
            return ExitCode::FAILURE;
        }
    };
    let json = values.str("LICTOR_OUTPUT") == "json";
    let result = runtime.block_on(async {
        if is_agent {
            agent_cmd::run(&values).await.map(|_| String::new())
        } else {
            commands::run(&path, &values).await
        }
    });
    match result {
        Ok(out) => {
            let mut stdout = std::io::stdout();
            let _ = stdout.write_all(out.as_bytes());
            let _ = stdout.flush();
            ExitCode::SUCCESS
        }
        Err(e) => {
            if json {
                let body = serde_json::json!({ "error": { "code": e.code, "message": e.message } });
                eprintln!("{body}");
            } else {
                eprintln!("Error: {}", e.message);
            }
            ExitCode::from(e.exit)
        }
    }
}
