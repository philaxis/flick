//! tidepool: tide tables for a harbour, read from the station's CSV.

use std::{env, fs, process::ExitCode};

mod tide;
use tide::{Reading, Station, Trend};

fn main() -> ExitCode {
    let args: Vec<String> = env::args().skip(1).collect();
    let station = match args.iter().position(|a| a == "--station") {
        Some(i) => args.get(i + 1).map(String::as_str).unwrap_or("incheon"),
        None => "incheon",
    };

    let path = format!("data/{station}.csv");
    let text = match fs::read_to_string(&path) {
        Ok(text) => text,
        Err(e) => {
            eprintln!("tidepool: can't read {path}: {e}");
            return ExitCode::FAILURE;
        }
    };

    let readings: Vec<Reading> = text.lines().skip(1).filter_map(Reading::parse).collect();
    let station = Station::new(station, &readings);

    println!("station  {}   ({})", station.name, station.position());
    println!("  time    height   trend");
    for turn in station.turns() {
        let mark = match turn.trend {
            Trend::High => "▲ high",
            Trend::Low => "▼ low",
        };
        println!("  {}   {:.2} m   {mark}", turn.at.format("%H:%M"), turn.height);
    }

    if let Some(next) = station.next_high() {
        println!("next high in {}", next.until_human());
    }
    ExitCode::SUCCESS
}
