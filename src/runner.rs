use std::io::{self, BufWriter, Read, Write};
use std::path::PathBuf;
use std::process::ExitCode;

use clap::Parser;

use anytomd::{
    ConversionOptions, ConversionWarning, convert_bytes, convert_file, gemini::GeminiDescriber,
};

enum Output {
    Stdout,
    File(BufWriter<std::fs::File>),
}

impl Write for Output {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        match self {
            Self::Stdout => io::stdout().lock().write(buf),
            Self::File(w) => w.write(buf),
        }
    }

    fn write_all(&mut self, buf: &[u8]) -> io::Result<()> {
        match self {
            Self::Stdout => io::stdout().lock().write_all(buf),
            Self::File(w) => w.write_all(buf),
        }
    }

    fn write_fmt(&mut self, fmt: std::fmt::Arguments<'_>) -> io::Result<()> {
        match self {
            Self::Stdout => io::stdout().lock().write_fmt(fmt),
            Self::File(w) => w.write_fmt(fmt),
        }
    }

    fn flush(&mut self) -> io::Result<()> {
        match self {
            Self::Stdout => io::stdout().lock().flush(),
            Self::File(w) => w.flush(),
        }
    }
}

fn open(path: &Option<PathBuf>) -> Result<Output, ExitCode> {
    match path {
        Some(p) => {
            let f = std::fs::File::create(p).map_err(|e| {
                eprintln!("error: {}: {e}", p.display());
                ExitCode::from(1)
            })?;
            Ok(Output::File(BufWriter::new(f)))
        }
        None => Ok(Output::Stdout),
    }
}

fn write_err(e: io::Error) -> ExitCode {
    eprintln!("error: output: {e}");
    ExitCode::from(1)
}

/// Convert various document formats to Markdown.
#[derive(Parser)]
#[command(version, about)]
struct Cli {
    /// Input files. Omit to read from stdin.
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

    /// Output plain text instead of Markdown.
    #[arg(long)]
    plain_text: bool,

    /// Maximum input file size (e.g., "500MB", "2GiB"). [default: 8GiB]
    #[arg(long, value_name = "SIZE", value_parser = crate::parse::byte_size)]
    max_input_size: Option<usize>,

    /// Maximum total extracted image bytes per document (e.g., "100MB", "1GiB"). [default: 4GiB]
    #[arg(long, value_name = "SIZE", value_parser = crate::parse::byte_size)]
    max_image_size: Option<usize>,

    /// Maximum total uncompressed size for ZIP-based documents (e.g., "1GiB", "8GiB"). [default: 16GiB]
    #[arg(long, value_name = "SIZE", value_parser = crate::parse::byte_size)]
    max_zip_size: Option<usize>,

    /// Use Gemini for image descriptions (requires GEMINI_API_KEY env var).
    #[arg(long)]
    gemini: bool,

    /// Gemini model to use. [default: gemini-3-flash-preview]
    #[arg(long, value_name = "MODEL", requires = "gemini")]
    gemini_model: Option<String>,
}

fn print_warnings(warnings: &[ConversionWarning]) {
    for w in warnings {
        match &w.location {
            Some(loc) => eprintln!("warning: [{:?}] {} ({loc})", w.code, w.message),
            None => eprintln!("warning: [{:?}] {}", w.code, w.message),
        }
    }
}

fn build_options(cli: &Cli) -> Result<ConversionOptions, ExitCode> {
    let d = ConversionOptions::default();
    let mut options = ConversionOptions {
        strict: cli.strict,
        max_input_bytes: cli.max_input_size.unwrap_or(d.max_input_bytes),
        max_total_image_bytes: cli.max_image_size.unwrap_or(d.max_total_image_bytes),
        max_uncompressed_zip_bytes: cli.max_zip_size.unwrap_or(d.max_uncompressed_zip_bytes),
        ..d
    };

    if cli.gemini {
        let describer = GeminiDescriber::from_env().map_err(|e| {
            eprintln!("error: --gemini: {e}");
            ExitCode::from(2)
        })?;
        options.image_describer = Some(std::sync::Arc::new(match cli.gemini_model {
            Some(ref model) => describer.with_model(model.clone()),
            None => describer,
        }));
    }

    Ok(options)
}

fn write_result(
    out: &mut impl Write,
    result: anytomd::ConversionResult,
    plain_text: bool,
) -> Result<(), ExitCode> {
    print_warnings(&result.warnings);
    let text = if plain_text {
        result.plain_text
    } else {
        result.markdown
    };
    out.write_all(text.as_bytes()).map_err(write_err)
}

fn convert(
    out: &mut impl Write,
    options: &ConversionOptions,
    files: &[PathBuf],
    format: Option<&str>,
    plain_text: bool,
) -> Result<(), ExitCode> {
    if files.is_empty() {
        let fmt = format.ok_or_else(|| {
            eprintln!("error: --format is required when reading from stdin");
            ExitCode::from(2)
        })?;

        let mut data = Vec::new();
        io::stdin().read_to_end(&mut data).map_err(|e| {
            eprintln!("error: stdin: {e}");
            ExitCode::from(1)
        })?;

        let result = convert_bytes(&data, fmt, options).map_err(|e| {
            eprintln!("error: stdin: {e}");
            ExitCode::from(1)
        })?;
        drop(data);
        write_result(out, result, plain_text)?;
        return Ok(());
    }

    let multiple = files.len() > 1;
    let mut had_error = false;

    for (i, path) in files.iter().enumerate() {
        if multiple && i > 0 {
            out.write_all(b"\n").map_err(write_err)?;
        }
        if multiple && !plain_text {
            write!(out, "<!-- source: {} -->\n\n", path.display()).map_err(write_err)?;
        }

        let result = if let Some(fmt) = format {
            let data = match std::fs::read(path) {
                Ok(d) => d,
                Err(e) => {
                    eprintln!("error: {}: {e}", path.display());
                    had_error = true;
                    continue;
                }
            };
            convert_bytes(&data, fmt, options)
        } else {
            convert_file(path, options)
        };

        match result {
            Ok(result) => write_result(out, result, plain_text)?,
            Err(e) => {
                eprintln!("error: {}: {e}", path.display());
                had_error = true;
            }
        }
    }

    if had_error {
        Err(ExitCode::from(1))
    } else {
        Ok(())
    }
}

fn run(cli: Cli) -> Result<(), ExitCode> {
    let options = build_options(&cli)?;
    let mut out = open(&cli.output)?;
    let result = convert(
        &mut out,
        &options,
        &cli.files,
        cli.format.as_deref(),
        cli.plain_text,
    );
    let flush_result = out.flush().map_err(write_err);
    result.and(flush_result)
}

pub(crate) fn main() -> ExitCode {
    match run(Cli::parse()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(code) => code,
    }
}
