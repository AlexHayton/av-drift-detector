use std::path::PathBuf;
use std::process::ExitCode;

use anyhow::{Context, Result, bail};
use av_drift_detector::{
    AnalyzeOptions, AudioCodec, CaptureMode, FrameRate, GenerateOptions, VideoCodec, parse_time,
};
use clap::{Args, Parser, Subcommand, ValueEnum};
use indicatif::{ProgressBar, ProgressStyle};

/// Generate machine-readable A/V sync test videos and measure audio/video drift.
#[derive(Parser)]
#[command(name = "av-drift", version, about)]
struct Cli {
    /// Show FFmpeg's own log output (hidden apart from errors by default).
    #[arg(short, long, global = true)]
    verbose: bool,
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Write a test video whose frames and audio both carry the frame number.
    Generate(GenerateArgs),
    /// Measure A/V drift frame by frame in a capture of a test video.
    #[command(alias = "check")]
    Analyze(AnalyzeArgs),
}

#[derive(Args)]
struct GenerateArgs {
    /// Frame rate: 30, 29.97, 30000/1001, ...
    #[arg(long, default_value = "30", value_parser = parse_fps)]
    fps: FrameRate,
    /// Duration (seconds, 1500ms, or HH:MM:SS).
    #[arg(long, default_value = "10", value_parser = parse_positive_time)]
    duration: f64,
    /// Frame size as WIDTHxHEIGHT.
    #[arg(long, default_value = "1280x720", value_parser = parse_size)]
    size: (u32, u32),
    /// Audio sample rate in Hz.
    #[arg(long, default_value_t = 48000)]
    sample_rate: u32,
    #[arg(long, value_enum, default_value_t = VideoCodecArg::H264)]
    video_codec: VideoCodecArg,
    #[arg(long, value_enum, default_value_t = AudioCodecArg::Aac)]
    audio_codec: AudioCodecArg,
    /// Deliberately delay the audio (e.g. 78.5ms, or -40ms for early audio).
    #[arg(long, default_value = "0", value_parser = parse_time_arg, allow_hyphen_values = true)]
    audio_offset: f64,
    /// Leave out the music under the code tones.
    #[arg(long)]
    no_music: bool,
    /// Output file (.mp4, .mov or .mkv).
    #[arg(short, long)]
    output: PathBuf,
}

#[derive(Args)]
struct AnalyzeArgs {
    /// The captured or re-encoded test video.
    input: PathBuf,
    /// Start of the analysis window (seconds, 1500ms, or HH:MM:SS).
    #[arg(long, default_value = "0", value_parser = parse_time_arg)]
    offset: f64,
    /// Length of the analysis window (default: to the end).
    #[arg(long, value_parser = parse_positive_time)]
    duration: Option<f64>,
    /// The test pattern's frame rate, if the stamps don't carry it.
    #[arg(long, value_parser = parse_fps)]
    fps: Option<FrameRate>,
    /// How the input was captured: `file` for re-encodes and recordings,
    /// `camera` for a camera filming a screen; `auto` decides from the stamps.
    #[arg(long, value_enum, default_value_t = CaptureArg::Auto)]
    capture: CaptureArg,
    /// Print a row for every frame.
    #[arg(long)]
    frames: bool,
    /// Print the full report as JSON.
    #[arg(long, conflicts_with = "csv")]
    json: bool,
    /// Print one CSV row per frame.
    #[arg(long)]
    csv: bool,
    /// Exit with status 2 if the absolute drift of any frame exceeds this (e.g. 45ms).
    #[arg(long, value_parser = parse_positive_time)]
    max_drift: Option<f64>,
    /// Don't fall back to reading the QR code when the barcode is unreadable.
    #[arg(long)]
    no_qr: bool,
}

#[derive(Clone, Copy, ValueEnum)]
enum CaptureArg {
    Auto,
    File,
    Camera,
}

#[derive(Clone, Copy, ValueEnum)]
enum VideoCodecArg {
    H264,
    Ffv1,
}

#[derive(Clone, Copy, ValueEnum)]
enum AudioCodecArg {
    Aac,
    Pcm,
}

fn parse_fps(s: &str) -> Result<FrameRate, String> {
    s.parse()
        .map_err(|e: av_drift_detector::Error| e.to_string())
}

fn parse_time_arg(s: &str) -> Result<f64, String> {
    parse_time(s).map_err(|e| e.to_string())
}

fn parse_positive_time(s: &str) -> Result<f64, String> {
    match parse_time(s) {
        Ok(t) if t > 0.0 => Ok(t),
        Ok(_) => Err("must be greater than zero".into()),
        Err(e) => Err(e.to_string()),
    }
}

fn parse_size(s: &str) -> Result<(u32, u32), String> {
    let (w, h) = s
        .split_once(['x', 'X'])
        .ok_or("expected WIDTHxHEIGHT, e.g. 1280x720")?;
    let parse = |v: &str| {
        v.trim()
            .parse::<u32>()
            .map_err(|_| format!("invalid size `{s}`"))
    };
    Ok((parse(w)?, parse(h)?))
}

fn main() -> ExitCode {
    match run(Cli::parse()) {
        Ok(code) => code,
        Err(e) => {
            eprintln!("error: {e:#}");
            ExitCode::FAILURE
        }
    }
}

fn run(cli: Cli) -> Result<ExitCode> {
    use ffmpeg_next::log::{Level, set_level};
    set_level(if cli.verbose {
        Level::Info
    } else {
        Level::Error
    });
    match cli.command {
        Command::Generate(args) => generate(args),
        Command::Analyze(args) => analyze(args),
    }
}

fn generate(args: GenerateArgs) -> Result<ExitCode> {
    let mut opts = GenerateOptions::new(&args.output, args.fps, args.duration);
    (opts.width, opts.height) = args.size;
    opts.sample_rate = args.sample_rate;
    opts.audio_delay = args.audio_offset;
    opts.music = !args.no_music;
    opts.video_codec = match args.video_codec {
        VideoCodecArg::H264 => VideoCodec::H264,
        VideoCodecArg::Ffv1 => VideoCodec::Ffv1,
    };
    opts.audio_codec = match args.audio_codec {
        AudioCodecArg::Aac => AudioCodec::Aac,
        AudioCodecArg::Pcm => AudioCodec::Pcm,
    };

    let bar = ProgressBar::new(opts.total_frames()).with_style(
        ProgressStyle::with_template("{spinner} [{bar:40}] {pos}/{len} frames ({eta})")
            .expect("progress template")
            .progress_chars("=> "),
    );
    av_drift_detector::generate(&opts, |p| bar.set_position(p.frames_done))
        .with_context(|| format!("generating {}", args.output.display()))?;
    bar.finish_and_clear();
    eprintln!(
        "Wrote {}: {} frames at {} fps ({:.3} s), {}x{}, audio offset {:+.2} ms",
        args.output.display(),
        opts.total_frames(),
        opts.fps.label(),
        opts.total_frames() as f64 * opts.fps.frame_duration(),
        opts.width,
        opts.height,
        opts.audio_delay * 1000.0
    );
    Ok(ExitCode::SUCCESS)
}

fn analyze(args: AnalyzeArgs) -> Result<ExitCode> {
    let opts = AnalyzeOptions {
        offset: args.offset,
        duration: args.duration,
        fps: args.fps,
        use_qr: !args.no_qr,
        capture: match args.capture {
            CaptureArg::Auto => CaptureMode::Auto,
            CaptureArg::File => CaptureMode::File,
            CaptureArg::Camera => CaptureMode::Camera,
        },
    };
    let report = av_drift_detector::analyze(&args.input, &opts)
        .with_context(|| format!("analysing {}", args.input.display()))?;

    if args.json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else if args.csv {
        print!("{}", report.to_csv());
    } else {
        if args.frames {
            println!("{}", report.frames_table());
        }
        println!("{}", report.summary_text());
    }

    if report.summary.measured == 0 {
        bail!("no frames could be measured (is this an av-drift-detector test video with audio?)");
    }
    if let Some(limit) = args.max_drift {
        let worst = [report.summary.min_ms, report.summary.max_ms]
            .into_iter()
            .flatten()
            .map(f64::abs)
            .fold(0.0, f64::max);
        if worst > limit * 1000.0 {
            eprintln!(
                "drift {worst:.2} ms exceeds the {:.2} ms limit",
                limit * 1000.0
            );
            return Ok(ExitCode::from(2));
        }
    }
    Ok(ExitCode::SUCCESS)
}
