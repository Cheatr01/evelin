use std::fs;
use std::io::IsTerminal;
use std::path::PathBuf;

use clap::{Args, Parser, Subcommand};
use evelin::config::ProjectLayout;
use evelin::error::{EvelinError, Result};
use evelin::eval::{run_eval, write_eval_report};
use evelin::gate::{discover_requirements, lint_items, load_requirements_from_file};
use evelin::schema;
use evelin::suite::{run_skill_suite, write_suite_report};

const LABEL_WIDTH: usize = 34;

#[derive(Debug, Parser)]
#[command(name = "evelin")]
#[command(about = "Rust evaluation toolkit for AI agent assets.")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Debug, Subcommand)]
enum Commands {
    SchemaLint(SchemaLintArgs),
    GateLint(GateLintArgs),
    Eval(EvalArgs),
    Suite(SuiteArgs),
}

#[derive(Debug, Args)]
struct SchemaLintArgs {
    #[arg(long)]
    config: PathBuf,
    #[arg(long)]
    out: PathBuf,
}

#[derive(Debug, Args)]
struct GateLintArgs {
    #[arg(long)]
    requirements: Option<PathBuf>,
    #[arg(long)]
    discover_dir: Option<PathBuf>,
    #[arg(long)]
    out: PathBuf,
    #[arg(long, default_value = ".")]
    root: PathBuf,
}

#[derive(Debug, Args)]
struct EvalArgs {
    #[arg(long)]
    config: PathBuf,
    #[arg(long)]
    out: PathBuf,
    #[arg(long, default_value = ".")]
    root: PathBuf,
}

#[derive(Debug, Args)]
struct SuiteArgs {
    #[arg(long)]
    skill: String,
    #[arg(long)]
    out_dir: Option<PathBuf>,
    #[arg(long)]
    summary_out: Option<PathBuf>,
    #[arg(long, default_value = ".")]
    root: PathBuf,
}

fn main() -> std::process::ExitCode {
    match run() {
        Ok(code) => std::process::ExitCode::from(code as u8),
        Err(error) => {
            eprintln!("{error}");
            std::process::ExitCode::from(1)
        }
    }
}

fn run() -> Result<i32> {
    let cli = Cli::parse();
    match cli.command {
        Commands::SchemaLint(args) => run_schema_lint(args),
        Commands::GateLint(args) => run_gate_lint(args),
        Commands::Eval(args) => run_eval_command(args),
        Commands::Suite(args) => run_suite_command(args),
    }
}

fn run_schema_lint(args: SchemaLintArgs) -> Result<i32> {
    let started = std::time::Instant::now();
    let value = evelin::document::load_document(&args.config)?;
    let errors = schema::validate_suite_document(&args.config, &value);
    let verdict = if errors.is_empty() { "pass" } else { "fail" };
    let duration_ms = started.elapsed().as_millis();
    write_json(
        &args.out,
        &serde_json::json!({
            "generated_at_utc": chrono::Utc::now(),
            "config_path": args.config.display().to_string(),
            "schema_path": "core/src/skill-suite.schema.yaml",
            "duration_ms": duration_ms,
            "summary": {
                "total": 1,
                "passed": if verdict == "pass" { 1 } else { 0 },
                "failed": if verdict == "pass" { 0 } else { 1 },
                "verdict": verdict,
                "duration_ms": duration_ms
            },
            "results": [{
                "config": args.config.display().to_string(),
                "pass": verdict == "pass",
                "errors": errors
            }]
        }),
    )?;
    let details = if verdict == "pass" {
        format!("schema ok  t={}ms", duration_ms)
    } else {
        format!("{} error(s)  t={}ms", errors.len(), duration_ms)
    };
    println!(
        "{} {:<LABEL_WIDTH$} {}",
        icon(verdict),
        "schema-lint",
        details,
        LABEL_WIDTH = LABEL_WIDTH
    );
    Ok(if verdict == "pass" { 0 } else { 1 })
}

fn run_gate_lint(args: GateLintArgs) -> Result<i32> {
    if args.requirements.is_some() == args.discover_dir.is_some() {
        return Err(EvelinError::message(
            "Provide exactly one of --requirements or --discover-dir",
        ));
    }
    let started = std::time::Instant::now();
    let layout = ProjectLayout::discover(args.root);
    let items = if let Some(path) = args.requirements {
        load_requirements_from_file(&layout.resolve(path.to_string_lossy()))?
    } else {
        discover_requirements(
            &layout.resolve(
                args.discover_dir
                    .as_ref()
                    .map(|path| path.to_string_lossy().to_string())
                    .unwrap_or_default(),
            ),
        )?
    };
    let report = lint_items(&layout, &items, started.elapsed().as_millis())?;
    write_json(&args.out, &report)?;
    println!(
        "{} {:<LABEL_WIDTH$} {}/{} checks  t={}ms",
        icon(&report.summary.verdict),
        "gate-lint",
        report.summary.passed,
        report.summary.total,
        report.duration_ms,
        LABEL_WIDTH = LABEL_WIDTH
    );
    Ok(if report.summary.verdict == "pass" {
        0
    } else {
        1
    })
}

fn run_eval_command(args: EvalArgs) -> Result<i32> {
    let layout = ProjectLayout::discover(args.root);
    let report = run_eval(&layout, &layout.resolve(args.config.to_string_lossy()))?;
    write_eval_report(&args.out, &report)?;
    println!(
        "{} {:<LABEL_WIDTH$} {}/{} pass  rate={:.3}  t={}ms",
        icon(&report.summary.verdict),
        format!(
            "eval:{}/{}",
            report.skill,
            PathBuf::from(&args.config)
                .file_name()
                .and_then(|v| v.to_str())
                .unwrap_or("eval")
        ),
        report.summary.passed,
        report.summary.total,
        report.summary.pass_rate,
        report.summary.duration_ms,
        LABEL_WIDTH = LABEL_WIDTH
    );
    Ok(if report.summary.verdict == "fail" {
        1
    } else {
        0
    })
}

fn run_suite_command(args: SuiteArgs) -> Result<i32> {
    let layout = ProjectLayout::discover(args.root);
    let out_dir = args.out_dir.unwrap_or_else(|| {
        layout
            .root
            .join("tests")
            .join("results")
            .join("skills")
            .join(&args.skill)
    });
    let summary_out = args
        .summary_out
        .unwrap_or_else(|| out_dir.join("suite-summary.json"));
    let report = run_skill_suite(&layout, &args.skill, &out_dir)?;
    write_suite_report(&summary_out, &report)?;

    println!("{} skill {}", icon("info"), report.skill);
    let step_type_width = report
        .steps
        .iter()
        .map(|step| suite_step_type_label(&step.step_type).len())
        .max()
        .unwrap_or(5)
        .max("runtime".len())
        .max("suite".len());
    let step_name_width = report
        .steps
        .iter()
        .map(|step| step.name.len())
        .max()
        .unwrap_or(5);
    for step in &report.steps {
        let detail = step
            .detail
            .clone()
            .or_else(|| step.reason.clone())
            .unwrap_or_default();
        println!(
            "  {} {:<step_type_width$} {:<step_name_width$} {}",
            icon(&step.status),
            suite_step_type_label(&step.step_type),
            step.name,
            detail,
            step_type_width = step_type_width,
            step_name_width = step_name_width
        );
        if let Some(runtime_detail) = &step.runtime_detail {
            println!(
                "  {} {:<step_type_width$} {:<step_name_width$} {}",
                icon("info"),
                "runtime",
                step.name,
                format_runtime_detail(runtime_detail),
                step_type_width = step_type_width,
                step_name_width = step_name_width
            );
        }
    }
    println!(
        "  {} {:<step_type_width$} {:<step_name_width$} {}",
        icon(&report.summary.verdict),
        "suite",
        "",
        format!(
            "pass={}  fail={}  skipped={}  t={}ms",
            report.summary.steps_passed,
            report.summary.steps_failed,
            report.summary.steps_skipped,
            report.summary.duration_ms
        ),
        step_type_width = step_type_width,
        step_name_width = step_name_width
    );
    Ok(if report.summary.verdict == "fail" {
        1
    } else {
        0
    })
}

fn suite_step_type_label(step_type: &str) -> &str {
    match step_type {
        "schema_lint" => "schema",
        "gate_lint" => "gate",
        other => other,
    }
}

fn format_runtime_detail(runtime_detail: &str) -> String {
    runtime_detail
        .split("  ")
        .filter(|segment| !segment.is_empty())
        .collect::<Vec<_>>()
        .join(" | ")
}

fn write_json<T: serde::Serialize>(path: &PathBuf, value: &T) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|source| EvelinError::Io {
            path: parent.to_path_buf(),
            source,
        })?;
    }
    fs::write(
        path,
        serde_json::to_string_pretty(value)
            .map_err(|error| EvelinError::message(format!("failed to serialize json: {error}")))?
            + "\n",
    )
    .map_err(|source| EvelinError::Io {
        path: path.to_path_buf(),
        source,
    })
}

fn icon(status: &str) -> String {
    match status {
        "pass" => paint("✓", "32"),
        "fail" => paint("✗", "31"),
        "skipped" => paint("•", "33"),
        "info" => paint("›", "36"),
        _ => "·".to_owned(),
    }
}

fn paint(text: &str, code: &str) -> String {
    if !std::io::stdout().is_terminal() || std::env::var_os("NO_COLOR").is_some() {
        return text.to_owned();
    }
    format!("\u{1b}[{code}m{text}\u{1b}[0m")
}

#[cfg(test)]
mod tests {
    use super::{format_runtime_detail, suite_step_type_label};

    #[test]
    fn shortens_suite_step_type_labels() {
        assert_eq!(suite_step_type_label("schema_lint"), "schema");
        assert_eq!(suite_step_type_label("gate_lint"), "gate");
        assert_eq!(suite_step_type_label("eval"), "eval");
    }

    #[test]
    fn compacts_runtime_detail_separators() {
        assert_eq!(
            format_runtime_detail("isolation=on  concurrency=3/3  retries=0  sandbox=read-only"),
            "isolation=on | concurrency=3/3 | retries=0 | sandbox=read-only"
        );
    }
}
