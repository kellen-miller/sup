use clap::{Args, Parser, Subcommand};
use std::collections::HashMap;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::PathBuf;

use crate::config::{self, AuthMode};
use crate::display;
use crate::runner::{self, Cancellation, RunOptions};

#[derive(Parser)]
#[command(name = "sup", about = "Run local development update jobs")]
struct Cli {
    #[arg(long, global = true)]
    config: Option<PathBuf>,
    #[arg(long, global = true, action = clap::ArgAction::Append, value_delimiter = ',')]
    only: Vec<String>,
    #[arg(long, global = true, action = clap::ArgAction::Append, value_delimiter = ',')]
    skip: Vec<String>,
    #[arg(long, global = true, default_value_t = 40)]
    tail: usize,
    #[arg(long, global = true)]
    log_retention_days: Option<i64>,
    #[arg(long, global = true)]
    no_log_cleanup: bool,
    #[arg(long, global = true)]
    no_tui: bool,
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand)]
enum Command {
    List,
    Plan,
    Run,
    Config(ConfigCommand),
}

#[derive(Args)]
struct ConfigCommand {
    #[command(subcommand)]
    command: ConfigAction,
}

#[derive(Subcommand)]
enum ConfigAction {
    Init {
        #[arg(long)]
        output: Option<PathBuf>,
    },
    Migrate {
        #[arg(long)]
        from: PathBuf,
        #[arg(long)]
        to: PathBuf,
    },
}

pub fn run<I, T>(args: I) -> i32
where
    I: IntoIterator<Item = T>,
    T: Into<std::ffi::OsString> + Clone,
{
    let cli = match Cli::try_parse_from(std::iter::once("sup".into()).chain(args.into_iter().map(Into::into))) {
        Ok(cli) => cli,
        Err(error) => {
            let code = error.exit_code();
            let _ = error.print();
            return code;
        }
    };
    let env: HashMap<String, String> = std::env::vars().collect();
    let home = match env.get("HOME") {
        Some(home) if !home.is_empty() => PathBuf::from(home),
        _ => {
            eprintln!("sup: cannot locate home directory: HOME is unset");
            return 2;
        }
    };
    if let Some(Command::Config(config_command)) = cli.command {
        return match config_command.command {
            ConfigAction::Init { output } => {
                let default_path = match config::resolve_path(cli.config.as_deref(), &env) {
                    Ok((path, _)) => path,
                    Err(error) => {
                        eprintln!("sup: {error}");
                        return 2;
                    }
                };
                let target = output.unwrap_or(default_path);
                create_exclusive(&target, config::DEFAULT_CONFIG.as_bytes())
            }
            ConfigAction::Migrate { from, to } => migrate(&from, &to, &home, &env),
        };
    }
    let loaded = match config::load(cli.config.as_deref(), &home, &env) {
        Ok(config) => config,
        Err(error) => {
            eprintln!("sup: {error}");
            return 2;
        }
    };
    let command = cli.command.unwrap_or(Command::Run);
    let selected = match config::selectors(&loaded.jobs, &loaded.aliases, &cli.only, &cli.skip) {
        Ok(jobs) => jobs,
        Err(error) => {
            eprintln!("sup: {error}");
            return 2;
        }
    };
    match command {
        Command::List => {
            for job in loaded.jobs {
                println!("{}", job.name);
            }
            0
        }
        Command::Plan => {
            println!("Config: {}", loaded.source);
            for job in selected {
                if job.auth == AuthMode::Preflight && direct_brew(&job.command) {
                    eprintln!(
                        "warning: {} directly mutates Homebrew with preflight auth; use on-demand",
                        job.name
                    );
                }
                println!(
                    "{} [{}] auth={:?}  {}",
                    job.name,
                    job.phase,
                    job.auth,
                    runner::displayed_command(&job)
                        .iter()
                        .map(|v| format!("{:?}", v))
                        .collect::<Vec<_>>()
                        .join(" ")
                );
            }
            0
        }
        Command::Run => {
            let cancellation = Cancellation::new();
            let options = RunOptions {
                home,
                retention_days: cli.log_retention_days.unwrap_or(loaded.retention_days),
                cleanup_logs: loaded.cleanup_logs && !cli.no_log_cleanup,
            };
            let (events_tx, events_rx) = std::sync::mpsc::sync_channel(128);
            let (auth_tx, auth_rx) = std::sync::mpsc::sync_channel(8);
            let worker_cancel = cancellation.clone();
            let worker_jobs = selected.clone();
            let worker =
                std::thread::spawn(move || runner::run(&worker_jobs, options, events_tx, auth_tx, worker_cancel));
            let displayed = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                display::run(&selected, events_rx, auth_rx, cancellation.clone(), cli.no_tui)
            }))
            .unwrap_or_else(|_| Err("dashboard panicked".to_owned()));
            if displayed.is_err() {
                cancellation.cancel();
            }
            match worker.join() {
                Ok(Ok(results)) => match displayed {
                    Ok(log_dir) => {
                        display::print_summary(&results, log_dir.as_deref(), cli.tail);
                        runner::exit_code(&results, &cancellation)
                    }
                    Err(error) => {
                        eprintln!("sup: dashboard failed: {error}");
                        display::print_summary(&results, None, cli.tail);
                        1
                    }
                },
                Ok(Err(error)) => {
                    eprintln!("sup: {error}");
                    1
                }
                Err(_) => {
                    eprintln!("sup: runner failed unexpectedly");
                    1
                }
            }
        }
        Command::Config(_) => unreachable!(),
    }
}

fn direct_brew(command: &[String]) -> bool {
    command
        .first()
        .is_some_and(|exe| std::path::Path::new(exe).file_name().and_then(|s| s.to_str()) == Some("brew"))
        && command
            .get(1)
            .is_some_and(|arg| matches!(arg.as_str(), "install" | "upgrade" | "reinstall"))
}

fn create_exclusive(path: &PathBuf, bytes: &[u8]) -> i32 {
    if let Some(parent) = path.parent()
        && !parent.as_os_str().is_empty()
        && let Err(error) = fs::create_dir_all(parent)
    {
        eprintln!("sup: cannot create {}: {error}", parent.display());
        return 2;
    }
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let temp = path.with_file_name(format!(".sup-{}-{nonce}.tmp", std::process::id()));
    let mut file = match OpenOptions::new().write(true).create_new(true).open(&temp) {
        Ok(file) => file,
        Err(error) => {
            eprintln!("sup: cannot create temporary output for {}: {error}", path.display());
            return 2;
        }
    };
    if let Err(error) = file
        .write_all(bytes)
        .and_then(|()| file.sync_all())
        .and_then(|()| fs::hard_link(&temp, path))
    {
        let _ = fs::remove_file(&temp);
        eprintln!("sup: cannot publish {} without overwriting: {error}", path.display());
        return 2;
    }
    if let Err(error) = fs::remove_file(&temp) {
        eprintln!(
            "sup: created {} but cannot remove temporary file {}: {error}",
            path.display(),
            temp.display()
        );
        return 2;
    }
    println!("Created {}", path.display());
    0
}

fn migrate(from: &PathBuf, to: &PathBuf, home: &std::path::Path, env: &HashMap<String, String>) -> i32 {
    if from == to {
        eprintln!("sup: migration input and output must be different paths");
        return 2;
    }
    let text = match fs::read_to_string(from) {
        Ok(text) => text,
        Err(error) => {
            eprintln!("sup: cannot read {}: {error}", from.display());
            return 2;
        }
    };
    let (toml, notices) = match config::migrate_yaml(&text, home, env) {
        Ok(result) => result,
        Err(error) => {
            eprintln!("sup: {error}");
            return 2;
        }
    };
    if let Err(error) = fs::canonicalize(from).and_then(|input| {
        let output = if to.exists() {
            fs::canonicalize(to)?
        } else {
            to.parent()
                .unwrap_or(std::path::Path::new("."))
                .canonicalize()?
                .join(to.file_name().unwrap_or_default())
        };
        if input == output {
            Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "input and output resolve to the same file",
            ))
        } else {
            Ok(())
        }
    }) {
        eprintln!("sup: {error}");
        return 2;
    }
    for notice in notices {
        eprintln!("notice: {notice}");
    }
    create_exclusive(to, toml.as_bytes())
}
