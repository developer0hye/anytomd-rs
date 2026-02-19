use std::io::{self, Read, Write};
use std::path::PathBuf;
use std::process::ExitCode;

use clap::Parser;

use anytomd::{ConversionOptions, ConversionWarning};

/// Convert various document formats to Markdown.
#[derive(Parser)]
#[command(version, about)]
struct Cli {
    /// Input files. Omit to read from stdin.
    #[arg()]
    files: Vec<PathBuf>,

    /// Write output to a file instead of stdout.
    #[arg(short, long, value_name = "PATH")]
    output: Option<PathBuf>,

    /// Format hint (e.g., html, csv). Required when reading from stdin.
    #[arg(short, long, value_name = "FMT")]
    format: Option<String>,

    /// Treat recoverable errors as hard errors.
    #[arg(long)]
    strict: bool,
}

fn print_warnings(warnings: &[ConversionWarning]) {
    for w in warnings {
        let loc = w
            .location
            .as_deref()
            .map(|l| format!(" ({l})"))
            .unwrap_or_default();
        eprintln!("warning: [{:?}] {}{}", w.code, w.message, loc);
    }
}

fn build_options(cli: &Cli) -> ConversionOptions {
    let options = ConversionOptions {
        strict: cli.strict,
        ..Default::default()
    };

    if let Ok(describer) = anytomd::gemini::GeminiDescriber::from_env() {
        eprintln!("info: using Gemini for image descriptions (GEMINI_API_KEY detected)");
        return ConversionOptions {
            image_describer: Some(std::sync::Arc::new(describer)),
            ..options
        };
    }

    options
}

fn run(cli: Cli) -> Result<ExitCode, ExitCode> {
    let options = build_options(&cli);

    let mut output_buf = String::new();
    let mut had_error = false;

    if cli.files.is_empty() {
        // Read from stdin
        let fmt = cli.format.as_deref().ok_or_else(|| {
            eprintln!("error: --format is required when reading from stdin");
            ExitCode::from(2)
        })?;

        let mut data = Vec::new();
        io::stdin().read_to_end(&mut data).map_err(|e| {
            eprintln!("error: stdin: {e}");
            ExitCode::from(1)
        })?;

        match anytomd::convert_bytes(&data, fmt, &options) {
            Ok(result) => {
                print_warnings(&result.warnings);
                output_buf.push_str(&result.markdown);
            }
            Err(e) => {
                eprintln!("error: stdin: {e}");
                return Err(ExitCode::from(1));
            }
        }
    } else {
        let multiple = cli.files.len() > 1;

        for (i, path) in cli.files.iter().enumerate() {
            // Insert separator between files
            if multiple && i > 0 {
                output_buf.push('\n');
            }
            if multiple {
                output_buf.push_str(&format!("<!-- source: {} -->\n\n", path.display()));
            }

            // If --format is given, use convert_bytes with that format override
            let result = if let Some(ref fmt) = cli.format {
                let data = match std::fs::read(path) {
                    Ok(d) => d,
                    Err(e) => {
                        eprintln!("error: {}: {e}", path.display());
                        had_error = true;
                        continue;
                    }
                };
                anytomd::convert_bytes(&data, fmt, &options)
            } else {
                anytomd::convert_file(path, &options)
            };

            match result {
                Ok(result) => {
                    print_warnings(&result.warnings);
                    output_buf.push_str(&result.markdown);
                }
                Err(e) => {
                    eprintln!("error: {}: {e}", path.display());
                    had_error = true;
                }
            }
        }
    }

    // Write output
    if let Some(ref out_path) = cli.output {
        std::fs::write(out_path, &output_buf).map_err(|e| {
            eprintln!("error: {}: {e}", out_path.display());
            ExitCode::from(1)
        })?;
    } else {
        io::stdout().write_all(output_buf.as_bytes()).map_err(|e| {
            eprintln!("error: stdout: {e}");
            ExitCode::from(1)
        })?;
    }

    if had_error {
        Err(ExitCode::from(1))
    } else {
        Ok(ExitCode::SUCCESS)
    }
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    match run(cli) {
        Ok(code) => code,
        Err(code) => code,
    }
}
