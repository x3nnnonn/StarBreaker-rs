use starbreaker_datacore::Database;
use std::{env, fs, time::Instant};

fn main() {
    let path = env::args().nth(1).expect("Usage: bench_export <path.dcb>");
    let data = fs::read(&path).expect("failed to read");

    let parse_start = Instant::now();
    let db = Database::from_bytes(&data).expect("failed to parse");
    let parse_time = parse_start.elapsed();
    println!("Parse time:  {:?}", parse_time);
    println!();

    bench("JSON", &db, |db, r| {
        starbreaker_datacore::export::to_json(db, r).map(|b| b.len())
    });
    bench("XML (sb)", &db, |db, r| {
        starbreaker_datacore::export::to_xml(db, r).map(|b| b.len())
    });
    bench("XML (unp4k)", &db, |db, r| {
        starbreaker_datacore::export::to_unp4k_xml(db, r).map(|b| b.len())
    });
    bench("XML (DataForge)", &db, |db, r| {
        starbreaker_datacore::export::to_dataforge_xml(db, r).map(|b| b.len())
    });
}

fn bench<F>(label: &str, db: &Database, f: F)
where
    F: Fn(&Database, &starbreaker_datacore::types::Record) -> Result<usize, starbreaker_datacore::error::ExportError>,
{
    let mut total_bytes = 0usize;
    let mut count = 0u32;
    let start = Instant::now();
    for record in db.records() {
        if !db.is_main_record(record) {
            continue;
        }
        if let Ok(n) = f(db, record) {
            total_bytes += n;
            count += 1;
        }
    }
    let elapsed = start.elapsed();
    println!("=== {label} ===");
    println!("  Export time: {:?} ({count} records)", elapsed);
    println!(
        "  Total bytes: {} ({:.1} MB)",
        total_bytes,
        total_bytes as f64 / 1_048_576.0
    );
    println!("  Records/sec: {:.0}", count as f64 / elapsed.as_secs_f64());
    println!(
        "  MB/sec:      {:.1}",
        total_bytes as f64 / 1_048_576.0 / elapsed.as_secs_f64()
    );
    println!();
}
