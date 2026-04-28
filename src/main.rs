use std::error::Error;
use std::io;
use std::sync::mpsc;
use std::thread;

use muteback::config::AppConfig;
use muteback::runtime::{RuntimeEvent, RuntimeHandle};

fn main() -> Result<(), Box<dyn Error>> {
    let config = match load_config() {
        Some(config) => config?,
        None => return Ok(()),
    };

    let (event_tx, event_rx) = mpsc::channel();
    let mut runtime = RuntimeHandle::start(config.clone(), event_tx)?;
    let event_printer = thread::spawn(move || print_runtime_events(event_rx));

    println!("MuteBack console prototype");
    println!("Mode: automatic speech detection");
    println!("Detector: Silero VAD + near-field guard + loopback rejection");
    println!(
        "Detector path: mono -> 16 kHz resample -> Silero -> near-field gate -> loopback gate"
    );
    println!(
        "Ducking level: {:.0}% of the previous volume",
        config.normalized_ducking_level() * 100.0
    );
    println!(
        "Transition: {}",
        if config.smooth_ducking {
            format!(
                "smooth (down {} ms, up {} ms)",
                config.duck_fade.as_millis(),
                config.restore_fade.as_millis()
            )
        } else {
            "instant".to_string()
        }
    );
    println!(
        "Restore mode: {}",
        if config.manual_restore {
            "manual (press Enter to restore and stop)"
        } else {
            "automatic after silence"
        }
    );
    println!("Behavior: lowers the default Windows output volume while you speak");
    println!("Press Enter to stop and restore the volume.");

    let mut line = String::new();
    let _ = io::stdin().read_line(&mut line);

    let _ = runtime.request_restore();
    runtime.stop();
    let _ = event_printer.join();
    println!("Stopped.");
    Ok(())
}

fn print_runtime_events(event_rx: mpsc::Receiver<RuntimeEvent>) {
    while let Ok(event) = event_rx.recv() {
        match event {
            RuntimeEvent::Started(info) => {
                println!("Microphone: {}", info.microphone);
                println!(
                    "Input config: {} Hz, {} channel(s), {}",
                    info.input_sample_rate, info.input_channels, info.input_sample_format
                );
            }
            RuntimeEvent::Ducked => println!("Speech detected -> ducking audio"),
            RuntimeEvent::Restored => println!("Speech ended -> restoring audio"),
            RuntimeEvent::Warning(message) => eprintln!("Audio processing warning: {message}"),
            RuntimeEvent::Error(message) => eprintln!("Audio processing error: {message}"),
            RuntimeEvent::Stopped => break,
        }
    }
}

fn load_config() -> Option<Result<AppConfig, Box<dyn Error>>> {
    let args = std::env::args().skip(1).collect::<Vec<_>>();

    if args.iter().any(|arg| arg == "--help" || arg == "-h") {
        print_usage();
        return None;
    }

    Some(AppConfig::from_cli_args(args).map_err(|error| error.into()))
}

fn print_usage() {
    println!("MuteBack console prototype");
    println!();
    println!("Usage:");
    println!("  muteback [--duck-level 10] [--transition smooth|instant]");
    println!();
    println!("Options:");
    println!("  --duck-level <0-100>       Background volume while speaking. Default: 10");
    println!("  --transition <mode>        smooth or instant. Default: smooth");
    println!("  --restore-mode <mode>      automatic or manual. Default: automatic");
    println!("  --duck-fade-ms <ms>        Smooth fade-down duration. Default: 180");
    println!("  --restore-fade-ms <ms>     Smooth restore duration. Default: 260");
}
